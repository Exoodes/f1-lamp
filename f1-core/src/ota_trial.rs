//! Firmware arriving over WiFi starts on trial: if it keeps restarting
//! before it proves healthy, the lamp goes back to the firmware it had.
//!
//! The device stores how many times the trial firmware has started. At
//! every boot, before anything that could crash, it asks [`on_boot`] what
//! to do with that number.

/// Boots a new firmware gets to prove itself; on the next one the lamp goes
/// back.
pub const TRIAL_BOOTS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootDecision {
    /// No firmware on trial.
    Normal,
    /// Start number `boot` (1-based) of the firmware on trial; store
    /// `boot` before going on.
    Trial { boot: u8 },
    /// Every trial boot ended in a restart: switch back.
    FallBack,
}

/// `started`: how often the firmware on trial has started so far, `None`
/// when nothing is on trial.
pub fn on_boot(started: Option<u8>) -> BootDecision {
    match started {
        None => BootDecision::Normal,
        Some(n) if n >= TRIAL_BOOTS => BootDecision::FallBack,
        Some(n) => BootDecision::Trial { boot: n + 1 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_on_trial_is_a_normal_boot() {
        assert_eq!(on_boot(None), BootDecision::Normal);
    }

    #[test]
    fn a_fresh_update_starts_its_first_trial_boot() {
        assert_eq!(on_boot(Some(0)), BootDecision::Trial { boot: 1 });
    }

    #[test]
    fn three_trial_boots_then_back() {
        let mut started = Some(0);
        let mut decisions = Vec::new();
        loop {
            let decision = on_boot(started);
            decisions.push(decision);
            match decision {
                BootDecision::Trial { boot } => started = Some(boot),
                _ => break,
            }
        }
        assert_eq!(
            decisions,
            [
                BootDecision::Trial { boot: 1 },
                BootDecision::Trial { boot: 2 },
                BootDecision::Trial { boot: 3 },
                BootDecision::FallBack,
            ]
        );
    }

    #[test]
    fn a_corrupt_large_count_goes_back_instead_of_overflowing() {
        assert_eq!(on_boot(Some(u8::MAX)), BootDecision::FallBack);
    }
}
