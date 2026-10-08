//! Firmware updates over WiFi: the new image is written to the app slot that
//! isn't running, checked, and made the boot slot. The running firmware is
//! never touched, so a failed upload leaves the lamp as it was.
//!
//! A new firmware then runs on trial (see `f1_core::ota_trial`): every boot
//! is counted in NVS, and a firmware that hasn't proved healthy within three
//! boots is switched back to the one in the other slot. A trial boot that
//! isn't healthy in time restarts itself (`trial_overdue`), so one that never
//! gets WiFi goes back too, without anyone pulling the plug.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard, OnceLock,
    },
    time::Duration,
};

use anyhow::{bail, Context};
use esp_idf_svc::{
    http::server::{EspHttpConnection, Request},
    nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault},
    ota::EspOta,
    sys::{
        esp, esp_app_desc_t, esp_app_get_description, esp_ota_get_next_update_partition,
        esp_ota_get_partition_description, esp_ota_get_running_partition,
        esp_ota_set_boot_partition,
    },
};
use f1_core::ota_trial::{on_boot, BootDecision, HEALTHY_WITHIN};

use crate::config::nvs::{NAMESPACE, OTA_FELL_BACK, OTA_TRIAL, OTA_TRIAL_IMAGE};

/// Opened at boot by `check_boot`, used by `receive` and `mark_healthy`.
static TRIAL: OnceLock<Mutex<EspNvs<NvsDefault>>> = OnceLock::new();
/// This boot is a trial boot that hasn't proved healthy yet.
static ON_TRIAL: AtomicBool = AtomicBool::new(false);

/// The first thing after the logger, before WiFi or anything else that could
/// crash: counts a trial boot, or switches back to the other slot after
/// three of them. Never returns when it switches back.
pub fn check_boot(partition: EspDefaultNvsPartition) -> anyhow::Result<()> {
    let nvs = EspNvs::new(partition, NAMESPACE, true).context("open NVS namespace")?;
    if nvs.remove(OTA_FELL_BACK).unwrap_or(false) {
        log::warn!("ota: the last update kept restarting; this is the firmware from before it");
    }

    let mut started = nvs.get_u8(OTA_TRIAL).ok().flatten();
    if started.is_some() && !trial_is_running(&nvs) {
        log::warn!(
            "ota: the firmware on trial isn't the one running (flashed over USB?); trial ended"
        );
        end_trial(&nvs)?;
        started = None;
    }
    match on_boot(started) {
        BootDecision::Normal => {}
        BootDecision::Trial { boot } => {
            nvs.set_u8(OTA_TRIAL, boot)
                .context("count the trial boot")?;
            log::info!("ota: new firmware on trial, boot {boot}");
            ON_TRIAL.store(true, Ordering::Relaxed);
        }
        BootDecision::FallBack => {
            // Cleared first: whatever happens next, no endless switching.
            end_trial(&nvs)?;
            match switch_to_other_slot() {
                Ok(()) => {
                    let _ = nvs.set_u8(OTA_FELL_BACK, 1);
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
    ON_TRIAL.store(false, Ordering::Relaxed);
    let Some(nvs) = trial_store() else { return };
    if end_trial(&nvs).unwrap_or(false) {
        log::info!("ota: new firmware is healthy, trial passed");
    }
}

fn trial_store() -> Option<MutexGuard<'static, EspNvs<NvsDefault>>> {
    TRIAL
        .get()
        .map(|nvs| nvs.lock().unwrap_or_else(|e| e.into_inner()))
}

type SavedTrial = (Option<u8>, Option<u64>);

fn saved_trial(nvs: &EspNvs<NvsDefault>) -> SavedTrial {
    (
        nvs.get_u8(OTA_TRIAL).ok().flatten(),
        nvs.get_u64(OTA_TRIAL_IMAGE).ok().flatten(),
    )
}

fn start_trial(nvs: &EspNvs<NvsDefault>, image: u64) -> anyhow::Result<()> {
    nvs.set_u64(OTA_TRIAL_IMAGE, image)
        .context("start the trial")?;
    nvs.set_u8(OTA_TRIAL, 0).context("start the trial")?;
    Ok(())
}

fn restore_trial(nvs: &EspNvs<NvsDefault>, (boot, image): SavedTrial) {
    let _ = match boot {
        Some(boot) => nvs.set_u8(OTA_TRIAL, boot),
        None => nvs.remove(OTA_TRIAL).map(drop),
    };
    let _ = match image {
        Some(image) => nvs.set_u64(OTA_TRIAL_IMAGE, image),
        None => nvs.remove(OTA_TRIAL_IMAGE).map(drop),
    };
}

/// Whether a trial was going on.
fn end_trial(nvs: &EspNvs<NvsDefault>) -> anyhow::Result<bool> {
    let ended = nvs.remove(OTA_TRIAL).context("end the trial")?;
    let _ = nvs.remove(OTA_TRIAL_IMAGE);
    Ok(ended)
}

/// A trial saved by older firmware has no image; it counts as the running one.
fn trial_is_running(nvs: &EspNvs<NvsDefault>) -> bool {
    nvs.get_u64(OTA_TRIAL_IMAGE)
        .ok()
        .flatten()
        .is_none_or(|image| image == running_image())
}

/// The start of an app's ELF SHA-256: tells two firmware builds apart.
fn image_id(desc: &esp_app_desc_t) -> u64 {
    let mut id = [0u8; 8];
    id.copy_from_slice(&desc.app_elf_sha256[..8]);
    u64::from_le_bytes(id)
}

fn running_image() -> u64 {
    // SAFETY: the running app's description lives in flash for the whole run.
    image_id(unsafe { &*esp_app_get_description() })
}

/// The image just written to the slot being updated, before it's checked.
fn update_image() -> anyhow::Result<u64> {
    // SAFETY: only reads the partition table; the entry is valid for the
    // whole run.
    let slot = unsafe { esp_ota_get_next_update_partition(std::ptr::null()) };
    if slot.is_null() {
        bail!("no app slot to update");
    }
    let mut desc = esp_app_desc_t::default();
    // SAFETY: `slot` is a valid partition entry and `desc` a local the call
    // writes into.
    esp!(unsafe { esp_ota_get_partition_description(slot, &mut desc) })
        .context("read the new image's description")?;
    Ok(image_id(&desc))
}

/// Whether this trial boot has had its time (`HEALTHY_WITHIN`) and still
/// isn't healthy, so the caller should restart: that counts the next trial
/// boot, and after the last one `check_boot` goes back.
pub fn trial_overdue(since_boot: Duration) -> bool {
    ON_TRIAL.load(Ordering::Relaxed) && since_boot >= HEALTHY_WITHIN
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

    // The new firmware starts on trial.
    let image = update_image()?;
    let previous = trial_store().map(|nvs| saved_trial(&nvs));
    let started = trial_store().map_or(Ok(()), |nvs| start_trial(&nvs, image));
    // Checks the whole image, then makes its slot the boot slot.
    let completed = started.and_then(|()| update.complete().context("the image doesn't check out"));
    if let Err(e) = completed {
        if let (Some(nvs), Some(previous)) = (trial_store(), previous) {
            restore_trial(&nvs, previous);
        }
        return Err(e);
    }
    log::info!("ota: done, {received} bytes; restarting into the new firmware");
    Ok(())
}
