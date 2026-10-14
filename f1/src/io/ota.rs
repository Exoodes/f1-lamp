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
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use esp_idf_svc::{
    http::server::{EspHttpConnection, Request},
    nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault},
    ota::EspOta,
    sys::{
        esp, esp_app_desc_t, esp_app_get_description, esp_fill_random,
        esp_ota_get_next_update_partition, esp_ota_get_partition_description,
        esp_ota_get_running_partition, esp_ota_get_state_partition, esp_ota_img_states_t,
        esp_ota_img_states_t_ESP_OTA_IMG_PENDING_VERIFY, esp_ota_mark_app_valid_cancel_rollback,
        esp_ota_set_boot_partition, EspError,
    },
};
use f1_core::{
    ota_auth::{Nonces, UploadCheck, NONCE_LEN},
    ota_trial::{self, on_boot, ArmError, BootDecision, Trial, TrialStore, HEALTHY_WITHIN},
};

use super::{storage::open_nvs, system};
use crate::config::{
    nvs::{OTA_FELL_BACK, OTA_TRIAL, OTA_TRIAL_IMAGE},
    OTA_KEY,
};

/// Opened at boot by `check_boot`, used by `receive` and `mark_healthy`.
static TRIAL: OnceLock<Mutex<EspNvs<NvsDefault>>> = OnceLock::new();
/// This boot is a trial boot that hasn't proved healthy yet.
static ON_TRIAL: AtomicBool = AtomicBool::new(false);

/// The first thing after the logger, before WiFi or anything else that could
/// crash: counts a trial boot, or switches back to the other slot after
/// three of them. Never returns when it switches back.
///
/// An NVS error is logged, never returned: failing here would restart the
/// lamp on every boot, with no web server to send a fix to.
pub fn check_boot(partition: EspDefaultNvsPartition) {
    match open_nvs(partition) {
        Ok(nvs) => {
            check_trial(&nvs);
            let _ = TRIAL.set(Mutex::new(nvs));
        }
        Err(e) => log::error!("ota: no trial check, updates start no trial: {e:#}"),
    }
    if let Err(e) = confirm_to_bootloader() {
        log::error!("ota: {e:#}");
    }
}

fn check_trial(nvs: &EspNvs<NvsDefault>) {
    if nvs.remove(OTA_FELL_BACK).unwrap_or(false) {
        log::warn!("ota: the last update kept restarting; this is the firmware from before it");
    }

    let mut trial = NvsTrial(nvs);
    let stored = trial.load();
    match on_boot(stored, running_image()) {
        BootDecision::Normal => {}
        BootDecision::NotThisFirmware => {
            log::warn!(
                "ota: the firmware on trial isn't the one running (flashed over USB?); trial ended"
            );
            if let Err(e) = ota_trial::end(&mut trial) {
                log::error!("ota: can't end the trial: {e}");
            }
        }
        BootDecision::Trial { boot } => {
            let counted = Trial {
                boots: Some(boot),
                ..stored
            };
            // An uncounted boot isn't on trial: restarting it when it isn't
            // healthy in time would never get to the fallback.
            match trial.store(counted) {
                Ok(()) => {
                    log::info!("ota: new firmware on trial, boot {boot}");
                    ON_TRIAL.store(true, Ordering::Relaxed);
                }
                Err(e) => log::error!("ota: can't count the trial boot, so no trial: {e}"),
            }
        }
        BootDecision::FallBack => {
            // Cleared first: whatever happens next, no endless switching. If
            // it can't be cleared, the other firmware would find the same
            // trial, so stay.
            if let Err(e) = ota_trial::end(&mut trial) {
                log::error!("ota: can't end the trial, staying on this firmware: {e}");
                return;
            }
            match switch_to_other_slot() {
                Ok(()) => {
                    let _ = nvs.set_u8(OTA_FELL_BACK, 1);
                    log::error!(
                        "ota: new firmware never got healthy; going back to the previous one"
                    );
                    system::restart();
                }
                // E.g. the other slot was half overwritten: nothing to go back
                // to, so carry on with what runs.
                Err(e) => log::error!("ota: can't go back to the previous firmware: {e:#}"),
            }
        }
    }
}

/// The bootloader sends a firmware that arrived over WiFi back if it restarts
/// before this, e.g. a crash before `check_boot`. From here on the trial
/// counted in NVS decides.
fn confirm_to_bootloader() -> anyhow::Result<()> {
    let mut state: esp_ota_img_states_t = 0;
    // SAFETY: the running partition is a partition table entry, valid for the
    // whole run, and `state` is a local the call writes into.
    let read =
        esp!(unsafe { esp_ota_get_state_partition(esp_ota_get_running_partition(), &mut state) });
    if read.is_err() || state != esp_ota_img_states_t_ESP_OTA_IMG_PENDING_VERIFY {
        return Ok(());
    }
    // SAFETY: no arguments; it only rewrites `otadata`.
    esp!(unsafe { esp_ota_mark_app_valid_cancel_rollback() })
        .context("confirm the firmware to the bootloader")?;
    log::info!("ota: confirmed to the bootloader");
    Ok(())
}

/// The firmware works well enough to receive the next update: the trial is
/// over.
pub fn mark_healthy() {
    ON_TRIAL.store(false, Ordering::Relaxed);
    let Some(nvs) = trial_store() else { return };
    if ota_trial::end(&mut NvsTrial(&nvs)).unwrap_or(false) {
        log::info!("ota: new firmware is healthy, trial passed");
    }
}

fn trial_store() -> Option<MutexGuard<'static, EspNvs<NvsDefault>>> {
    TRIAL
        .get()
        .map(|nvs| nvs.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The trial in NVS.
struct NvsTrial<'a>(&'a EspNvs<NvsDefault>);

impl TrialStore for NvsTrial<'_> {
    type Error = EspError;

    fn load(&self) -> Trial {
        Trial {
            boots: self.0.get_u8(OTA_TRIAL).ok().flatten(),
            image: self.0.get_u64(OTA_TRIAL_IMAGE).ok().flatten(),
        }
    }

    /// The image is written before the count and removed after it: a count
    /// without an image counts as the running firmware's (`on_boot`), so a
    /// write cut in half never puts the wrong firmware on trial.
    fn store(&mut self, trial: Trial) -> Result<(), EspError> {
        if let Some(image) = trial.image {
            self.0.set_u64(OTA_TRIAL_IMAGE, image)?;
        }
        match trial.boots {
            Some(boots) => self.0.set_u8(OTA_TRIAL, boots)?,
            None => {
                self.0.remove(OTA_TRIAL)?;
            }
        }
        if trial.image.is_none() {
            self.0.remove(OTA_TRIAL_IMAGE)?;
        }
        Ok(())
    }
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

static NONCES: Mutex<Nonces> = Mutex::new(Nonces::new());

/// A new one-time nonce for the next upload, in hex.
pub fn issue_nonce() -> String {
    let mut random = [0u8; NONCE_LEN];
    // SAFETY: writes exactly `random.len()` bytes into the local buffer.
    unsafe { esp_fill_random(random.as_mut_ptr().cast(), random.len()) };
    NONCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .issue(random, Instant::now())
}

/// The nonce an upload names, if it's the one handed out and still fresh.
/// Used up either way.
pub fn redeem_nonce(hex: &str) -> Option<[u8; NONCE_LEN]> {
    NONCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .redeem(hex, Instant::now())
}

/// One app slot in partitions.csv (`0x1F0000`).
pub const MAX_IMAGE: usize = 0x1F_0000;
/// Read and written at a time.
const CHUNK: usize = 4096;

/// Writes the request body (`len` bytes, a `.bin` from `espflash
/// save-image`) to the free slot and, if `tag` proves the update key (see
/// `f1_core::ota_auth`), makes it the one to boot. The caller restarts the
/// chip afterwards.
pub fn receive(
    req: &mut Request<&mut EspHttpConnection<'_>>,
    len: usize,
    mut check: UploadCheck,
    tag: &str,
) -> anyhow::Result<()> {
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
        check.update(&buf[..n]);
        received += n;
        if received >= next_report && received < len {
            log::info!("ota: {}%", received * 100 / len);
            next_report += len / 4;
        }
    }

    if !check.verify(OTA_KEY, tag) {
        bail!("wrong update key, or the image changed on the way");
    }
    // The new firmware starts on trial, then its slot becomes the boot slot.
    let image = update_image()?;
    let complete = move || update.complete();
    let armed = match trial_store() {
        Some(nvs) => ota_trial::arm(&mut NvsTrial(&nvs), image, complete),
        None => complete().map_err(ArmError::Switch),
    };
    armed.map_err(|e| match e {
        ArmError::Store(e) => anyhow::Error::from(e).context("start the trial"),
        ArmError::Switch(e) => anyhow::Error::from(e).context("the image doesn't check out"),
    })?;
    log::info!("ota: done, {received} bytes; restarting into the new firmware");
    Ok(())
}
