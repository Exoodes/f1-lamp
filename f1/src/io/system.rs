//! Readings of the chip itself.

use std::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use esp_idf_svc::{hal::delay::TickType, sys};

/// The heap, in bytes.
pub struct Heap {
    pub free: u32,
    /// The least that was free at any moment since boot.
    pub lowest: u32,
    /// The largest block that could be allocated now: with free memory in
    /// small pieces, a large allocation (a TLS buffer, a big frame) fails
    /// even though `free` looks fine.
    pub largest_block: usize,
}

/// The least stack a thread had left since it started, in bytes, as the
/// thread itself last measured it: a thread can only be measured from
/// inside (std threads have no FreeRTOS handle to look up).
pub struct StackLeft(AtomicU32);

pub static MAIN_STACK: StackLeft = StackLeft::new();
pub static NET_STACK: StackLeft = StackLeft::new();
pub static LIVE_STACK: StackLeft = StackLeft::new();

impl StackLeft {
    const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Measures the calling thread. Scans its unused stack, so call it now
    /// and then, not every frame.
    pub fn measure(&self) {
        // SAFETY: a null handle means the calling task. ESP-IDF counts
        // stacks in bytes, not the words of FreeRTOS's documentation.
        let left = unsafe { sys::uxTaskGetStackHighWaterMark(ptr::null_mut()) };
        self.0.store(left, Ordering::Relaxed);
    }

    /// `None` until the thread has measured itself.
    pub fn get(&self) -> Option<u32> {
        Some(self.0.load(Ordering::Relaxed)).filter(|&left| left > 0)
    }
}

/// Entries in the NVS partition (32 B each): settings, the schedule, the
/// F1TV token and the update trial all share its 24 KB.
pub struct NvsUse {
    pub used: usize,
    pub total: usize,
}

pub fn nvs_use() -> Option<NvsUse> {
    let mut stats = sys::nvs_stats_t::default();
    // SAFETY: a null name means the default partition; `stats` is a local
    // the call fills in.
    sys::esp!(unsafe { sys::nvs_get_stats(ptr::null(), &mut stats) }).ok()?;
    Some(NvsUse {
        used: stats.used_entries,
        total: stats.total_entries,
    })
}

pub fn restart() -> ! {
    // SAFETY: `esp_restart` takes no arguments and has no preconditions; it
    // reboots the chip and never returns.
    unsafe { sys::esp_restart() }
}

/// Wakes the calling task every `period`, counted in FreeRTOS ticks.
/// `thread::sleep` can't do this: since ESP-IDF 5.2.x it sleeps at least the
/// time asked and so wakes a tick late, which made 20 ms frames 30 ms.
pub struct Pacer {
    last_wake: sys::TickType_t,
    period: sys::TickType_t,
}

impl Pacer {
    pub fn new(period: Duration) -> Self {
        Pacer {
            // SAFETY: only reads the scheduler's tick count.
            last_wake: unsafe { sys::xTaskGetTickCount() },
            period: TickType::from(period).ticks(),
        }
    }

    /// Waits until the next period starts; at once if this one is over.
    pub fn wait(&mut self) {
        // SAFETY: `last_wake` is a field that lives through the call, which
        // advances it by one period.
        unsafe { sys::xTaskDelayUntil(&mut self.last_wake, self.period) };
    }
}

/// Sets the calling task's FreeRTOS priority.
pub fn set_own_priority(priority: u32) {
    // SAFETY: a null handle means the calling task, which exists while it
    // calls; `priority` is clamped by FreeRTOS to its highest priority.
    unsafe { sys::vTaskPrioritySet(std::ptr::null_mut(), priority) }
}

/// Restarts the chip when the calling task goes `timeout` without
/// [`feed_watchdog`]. Only this task is watched: the idle task, which a long
/// TLS handshake can keep from running for seconds, no longer is.
pub fn watch_own_task(timeout: Duration) -> anyhow::Result<()> {
    let config = sys::esp_task_wdt_config_t {
        timeout_ms: u32::try_from(timeout.as_millis())?,
        idle_core_mask: 0,
        trigger_panic: true,
    };
    // SAFETY: the watchdog is set up at boot (`CONFIG_ESP_TASK_WDT_INIT`)
    // and `config` lives through the call.
    sys::esp!(unsafe { sys::esp_task_wdt_reconfigure(&config) })?;
    // SAFETY: a null handle means the calling task, which exists while it
    // calls.
    sys::esp!(unsafe { sys::esp_task_wdt_add(std::ptr::null_mut()) })?;
    Ok(())
}

pub fn feed_watchdog() {
    // SAFETY: only resets the calling task's watchdog entry.
    unsafe { sys::esp_task_wdt_reset() };
}

pub fn heap() -> Heap {
    // SAFETY: all three only read ESP-IDF's heap counters; they have no
    // preconditions.
    unsafe {
        Heap {
            free: sys::esp_get_free_heap_size(),
            lowest: sys::esp_get_minimum_free_heap_size(),
            largest_block: sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_8BIT),
        }
    }
}
