use std::time::{Duration, Instant};

use anyhow::Context;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use f1_core::{
    schedule::{self, Session},
    settings::{self, Settings},
};

use crate::config::nvs::{NAMESPACE, SCHEDULE, SETTINGS};

/// Flash wears out with writes, so changes are saved at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(5);

/// Keeps `Settings` in flash as JSON, so they survive a reboot.
pub struct SettingsStore {
    nvs: EspNvs<NvsDefault>,
    /// Changed but not yet saved.
    pending: Option<Settings>,
    last_saved: Instant,
}

impl SettingsStore {
    pub fn new(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        Ok(Self {
            nvs: EspNvs::new(partition, NAMESPACE, true).context("open NVS namespace")?,
            pending: None,
            last_saved: Instant::now(),
        })
    }

    /// The saved settings, or the defaults when none are saved or they can't be read.
    pub fn load(&self) -> Settings {
        let mut buf = [0u8; settings::MAX_JSON];
        match self.nvs.get_str(SETTINGS, &mut buf) {
            Ok(Some(json)) => serde_json::from_str(json)
                .inspect(|settings| log::info!("settings loaded: {settings:?}"))
                .unwrap_or_else(|e| {
                    log::warn!("saved settings are broken ({e}), using defaults");
                    Settings::default()
                }),
            Ok(None) => {
                log::info!("no saved settings, using defaults");
                Settings::default()
            }
            Err(e) => {
                log::warn!("can't read saved settings ({e}), using defaults");
                Settings::default()
            }
        }
    }

    /// Remembers `settings` to be saved by the next due `save_if_due`.
    pub fn changed(&mut self, settings: Settings) {
        self.pending = Some(settings);
    }

    /// Saves pending changes, unless the last save was too recent.
    pub fn save_if_due(&mut self, now: Instant) {
        if now - self.last_saved < SAVE_EVERY {
            return;
        }
        let Some(settings) = self.pending.take() else {
            return;
        };

        let saved = serde_json::to_string(&settings)
            .context("serialize settings")
            .and_then(|json| self.nvs.set_str(SETTINGS, &json).context("write settings"));
        match saved {
            Ok(()) => {
                self.last_saved = now;
                log::info!("settings saved");
            }
            Err(e) => {
                log::warn!("{e:#}, will retry");
                self.pending = Some(settings);
            }
        }
    }
}

/// Keeps the session schedule in flash, so a reboot during a session (when
/// OpenF1 refuses free requests) still knows what's on.
pub struct ScheduleStore {
    nvs: EspNvs<NvsDefault>,
}

impl ScheduleStore {
    pub fn new(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        Ok(Self {
            nvs: EspNvs::new(partition, NAMESPACE, true).context("open NVS namespace")?,
        })
    }

    /// The saved sessions; empty if none are saved or they can't be read.
    pub fn load(&self) -> Vec<Session> {
        let mut buf = [0u8; schedule::MAX_JSON];
        match self.nvs.get_str(SCHEDULE, &mut buf) {
            Ok(Some(json)) => serde_json::from_str(json)
                .inspect(|sessions: &Vec<Session>| {
                    log::info!("schedule loaded: {} sessions", sessions.len())
                })
                .unwrap_or_else(|e| {
                    log::warn!("saved schedule is broken ({e}), starting empty");
                    Vec::new()
                }),
            Ok(None) => {
                log::info!("no saved schedule");
                Vec::new()
            }
            Err(e) => {
                log::warn!("can't read saved schedule ({e}), starting empty");
                Vec::new()
            }
        }
    }

    /// Saves `sessions`, replacing what was saved before. Not throttled: the
    /// caller saves only when the schedule changed.
    pub fn save(&mut self, sessions: &[Session]) -> anyhow::Result<()> {
        let json = serde_json::to_string(sessions).context("serialize schedule")?;
        self.nvs
            .set_str(SCHEDULE, &json)
            .context("write schedule")?;
        Ok(())
    }
}
