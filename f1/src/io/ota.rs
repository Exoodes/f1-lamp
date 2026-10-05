//! Firmware updates over WiFi: the new image is written to the app slot that
//! isn't running, checked, and made the boot slot. The running firmware is
//! never touched, so a failed upload leaves the lamp as it was.
//!
//! A new firmware then runs on trial (see `f1_core::ota_trial`): every boot
//! is counted in NVS, and a firmware that hasn't proved healthy within three
//! boots is switched back to the one in the other slot.

use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context};
use esp_idf_svc::{
    http::server::{EspHttpConnection, Request},
    nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault},
    ota::EspOta,
    sys::{
        esp, esp_ota_get_next_update_partition, esp_ota_get_running_partition,
        esp_ota_set_boot_partition,
    },
};
use f1_core::ota_trial::{on_boot, BootDecision};

/// NVS limits names to 15 characters.
const NAMESPACE: &str = "f1";
/// How often the firmware on trial has started; absent when none is.
const TRIAL_KEY: &str = "ota_trial";
/// Set before switching back, so the firmware that runs next can say why.
const FELL_BACK_KEY: &str = "ota_fell_back";

/// Opened at boot by `check_boot`, used by `receive` and `mark_healthy`.
static TRIAL: OnceLock<Mutex<EspNvs<NvsDefault>>> = OnceLock::new();

/// The first thing after the logger, before WiFi or anything else that could
/// crash: counts a trial boot, or switches back to the other slot after
/// three of them. Never returns when it switches back.
pub fn check_boot(partition: EspDefaultNvsPartition) -> anyhow::Result<()> {
    let nvs = EspNvs::new(partition, NAMESPACE, true).context("open NVS namespace")?;
    if nvs.remove(FELL_BACK_KEY).unwrap_or(false) {
        log::warn!("ota: the last update kept restarting; this is the firmware from before it");
    }

    match on_boot(nvs.get_u8(TRIAL_KEY).ok().flatten()) {
        BootDecision::Normal => {}
        BootDecision::Trial { boot } => {
            nvs.set_u8(TRIAL_KEY, boot)
                .context("count the trial boot")?;
            log::info!("ota: new firmware on trial, boot {boot}");
        }
        BootDecision::FallBack => {
            // Cleared first: whatever happens next, no endless switching.
            nvs.remove(TRIAL_KEY).context("end the trial")?;
            match switch_to_other_slot() {
                Ok(()) => {
                    let _ = nvs.set_u8(FELL_BACK_KEY, 1);
                    log::error!(
                        "ota: new firmware never got healthy; going back to the previous one"
                    );
                    crate::restart();
                }
                // E.g. the other slot was half overwritten: nothing to go back
                // to, so carry on with what runs.
                Err(e) => log::error!("ota: can't go back to the previous firmware: {e:#}"),
            }
        }
    }
    let _ = TRIAL.set(Mutex::new(nvs));
    Ok(())
}

/// The firmware works well enough to receive the next update: the trial is
/// over.
pub fn mark_healthy() {
    let Some(nvs) = TRIAL.get() else { return };
    let nvs = nvs.lock().unwrap_or_else(|e| e.into_inner());
    if nvs.remove(TRIAL_KEY).unwrap_or(false) {
        log::info!("ota: new firmware is healthy, trial passed");
    }
}

/// Boots the other app slot next time; checks its image first.
fn switch_to_other_slot() -> anyhow::Result<()> {
    // SAFETY: both only read the partition table; with two app slots the
    // "next update partition" after the running one is the other slot.
    let other = unsafe { esp_ota_get_next_update_partition(esp_ota_get_running_partition()) };
    if other.is_null() {
        bail!("no other app slot");
    }
    // SAFETY: `other` is a partition table entry, valid for the program's
    // whole run; the call verifies the image in it before switching.
    esp!(unsafe { esp_ota_set_boot_partition(other) }).context("set the boot slot")?;
    Ok(())
}

/// One app slot in partitions.csv (`0x1F0000`).
pub const MAX_IMAGE: usize = 0x1F_0000;
/// Read and written at a time.
const CHUNK: usize = 4096;

/// Writes the request body (`len` bytes, a `.bin` from `espflash
/// save-image`) to the free slot and makes it the one to boot. The caller
/// restarts the chip afterwards.
pub fn receive(req: &mut Request<&mut EspHttpConnection<'_>>, len: usize) -> anyhow::Result<()> {
    if len == 0 || len > MAX_IMAGE {
        bail!("{len} bytes: a firmware image must be 1 to {MAX_IMAGE} bytes");
    }
    // Only one instance can exist, so a second upload at the same time fails here.
    let mut ota = EspOta::new().context("another update is running")?;
    // Erases only as much of the slot as the image needs.
    let mut update = ota
        .initiate_update_with_known_size(len)
        .context("start the update")?;
    log::info!("ota: receiving {len} bytes");

    let mut buf = vec![0u8; CHUNK];
    let mut received = 0;
    let mut next_report = len / 4;
    while received < len {
        let n = req.read(&mut buf).context("read the upload")?;
        if n == 0 {
            // `update` is dropped here, which aborts it: the slot stays unused.
            bail!("upload ended after {received} of {len} bytes");
        }
        // The first write also checks that this is an ESP32 image at all.
        update
            .write(&buf[..n])
            .with_context(|| format!("write at byte {received} (not firmware for this chip?)"))?;
        received += n;
        if received >= next_report && received < len {
            log::info!("ota: {}%", received * 100 / len);
            next_report += len / 4;
        }
    }

    // Checks the whole image, then makes its slot the boot slot.
    update.complete().context("the image doesn't check out")?;
    // The new firmware starts on trial.
    if let Some(nvs) = TRIAL.get() {
        let nvs = nvs.lock().unwrap_or_else(|e| e.into_inner());
        nvs.set_u8(TRIAL_KEY, 0).context("start the trial")?;
    }
    log::info!("ota: done, {received} bytes; restarting into the new firmware");
    Ok(())
}
