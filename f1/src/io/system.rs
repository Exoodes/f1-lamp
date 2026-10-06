//! Readings of the chip itself.

use esp_idf_svc::sys;

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

/// Sets the calling task's FreeRTOS priority.
pub fn set_own_priority(priority: u32) {
    // SAFETY: a null handle means the calling task, which exists while it
    // calls; `priority` is clamped by FreeRTOS to its highest priority.
    unsafe { sys::vTaskPrioritySet(std::ptr::null_mut(), priority) }
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
