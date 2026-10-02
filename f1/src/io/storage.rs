use std::time::{Duration, Instant};

use anyhow::Context;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use f1_core::settings::Settings;

/// NVS namespace and key; NVS limits both to 15 characters.
const NAMESPACE: &str = "f1";
const KEY: &str = "settings";
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
        let mut buf = [0u8; 512];
        match self.nvs.get_str(KEY, &mut buf) {
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
            .and_then(|json| self.nvs.set_str(KEY, &json).context("write settings"));
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
