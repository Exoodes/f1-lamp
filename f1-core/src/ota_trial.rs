//! Firmware arriving over WiFi starts on trial: if it keeps restarting
//! before it proves healthy, the lamp goes back to the firmware it had.
//!
//! The device stores how many times the trial firmware has started and which
//! image is on trial ([`Trial`], in a [`TrialStore`]). An upload puts the new
//! image on trial before making it the boot slot ([`arm`]). At every boot,
//! before anything that could crash, the device asks [`on_boot`] what to do.

use std::time::Duration;

/// Boots a new firmware gets to prove itself; on the next one the lamp goes
/// back.
pub const TRIAL_BOOTS: u8 = 3;

/// A trial boot that isn't healthy this long after starting restarts itself,
/// which counts as its next boot. Without it, a firmware that never gets
/// WiFi (and so never gets healthy) would wait forever instead of going back.
/// Healthy takes WiFi plus a minute of web server, so this leaves two
/// minutes for WiFi.
pub const HEALTHY_WITHIN: Duration = Duration::from_secs(3 * 60);

/// What's stored about the firmware on trial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trial {
    /// How often it has started so far; `None` when nothing is on trial.
    pub boots: Option<u8>,
    /// Which image it is (the start of its ELF SHA-256). `None` for a trial
    /// saved by firmware from before this was stored.
    pub image: Option<u64>,
}

/// Where the device keeps the [`Trial`].
pub trait TrialStore {
    type Error;

    fn load(&self) -> Trial;

    /// Replaces what's stored; a `None` field removes that entry.
    fn store(&mut self, trial: Trial) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootDecision {
    /// No firmware on trial.
    Normal,
    /// Start number `boot` (1-based) of the firmware on trial; store
    /// `boot` before going on.
    Trial { boot: u8 },
    /// Every trial boot ended in a restart: switch back.
    FallBack,
    /// A trial is stored, but for another image: this firmware came some
    /// other way (a USB flash, or the bootloader went back). End the trial.
    NotThisFirmware,
}

/// What to do at boot with the stored `trial`, for the firmware whose image
/// is `running_image`.
pub fn on_boot(trial: Trial, running_image: u64) -> BootDecision {
    let Some(started) = trial.boots else {
        return BootDecision::Normal;
    };
    if trial.image.is_some_and(|image| image != running_image) {
        return BootDecision::NotThisFirmware;
    }
    if started >= TRIAL_BOOTS {
        BootDecision::FallBack
    } else {
        BootDecision::Trial { boot: started + 1 }
    }
}

/// Why [`arm`] failed: storing the trial, or switching the boot slot.
#[derive(Debug, PartialEq, Eq)]
pub enum ArmError<S, W> {
    Store(S),
    Switch(W),
}

/// Puts `image` on trial, then calls `switch` to make its slot the boot slot.
/// In that order, so the new firmware can never boot without its trial. If
/// either step fails the stored trial goes back to what it was: a failed
/// upload leaves the running firmware's own trial as it was.
pub fn arm<S: TrialStore, W>(
    store: &mut S,
    image: u64,
    switch: impl FnOnce() -> Result<(), W>,
) -> Result<(), ArmError<S::Error, W>> {
    let previous = store.load();
    let new = Trial {
        boots: Some(0),
        image: Some(image),
    };
    let armed = store
        .store(new)
        .map_err(ArmError::Store)
        .and_then(|()| switch().map_err(ArmError::Switch));
    if armed.is_err() {
        let _ = store.store(previous);
    }
    armed
}

/// Ends the trial, whether it passed or the lamp goes back. Whether one was
/// going on.
pub fn end<S: TrialStore>(store: &mut S) -> Result<bool, S::Error> {
    let was_on_trial = store.load().boots.is_some();
    store.store(Trial::default())?;
    Ok(was_on_trial)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    const RUNNING: u64 = 0x1111;
    const NEW: u64 = 0x2222;

    fn on_trial(boots: u8, image: Option<u64>) -> Trial {
        Trial {
            boots: Some(boots),
            image,
        }
    }

    /// A store in memory that can be told to fail. Shared, so a test can
    /// look at it from inside `arm`'s switch.
    #[derive(Default)]
    struct Memory {
        trial: Rc<Cell<Trial>>,
        failing: bool,
    }

    impl Memory {
        fn holding(trial: Trial) -> Self {
            Memory {
                trial: Rc::new(Cell::new(trial)),
                failing: false,
            }
        }
    }

    impl TrialStore for Memory {
        type Error = &'static str;

        fn load(&self) -> Trial {
            self.trial.get()
        }

        fn store(&mut self, trial: Trial) -> Result<(), &'static str> {
            if self.failing {
                return Err("flash broken");
            }
            self.trial.set(trial);
            Ok(())
        }
    }

    #[test]
    fn nothing_on_trial_is_a_normal_boot() {
        assert_eq!(on_boot(Trial::default(), RUNNING), BootDecision::Normal);
    }

    #[test]
    fn a_fresh_update_starts_its_first_trial_boot() {
        assert_eq!(
            on_boot(on_trial(0, Some(RUNNING)), RUNNING),
            BootDecision::Trial { boot: 1 }
        );
    }

    #[test]
    fn three_trial_boots_then_back() {
        let mut trial = on_trial(0, Some(RUNNING));
        let mut decisions = Vec::new();
        loop {
            let decision = on_boot(trial, RUNNING);
            decisions.push(decision);
            match decision {
                BootDecision::Trial { boot } => trial.boots = Some(boot),
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
        assert_eq!(
            on_boot(on_trial(u8::MAX, Some(RUNNING)), RUNNING),
            BootDecision::FallBack
        );
    }

    #[test]
    fn a_trial_of_another_image_ends_instead_of_going_back() {
        // The bad update crashed three times, then came a USB flash.
        assert_eq!(
            on_boot(on_trial(TRIAL_BOOTS, Some(NEW)), RUNNING),
            BootDecision::NotThisFirmware
        );
    }

    #[test]
    fn a_trial_without_an_image_counts_as_the_running_one() {
        assert_eq!(
            on_boot(on_trial(0, None), RUNNING),
            BootDecision::Trial { boot: 1 }
        );
    }

    #[test]
    fn arming_stores_the_trial_before_switching() {
        let mut store = Memory::default();
        let stored = Rc::clone(&store.trial);
        let mut stored_when_switching = None;
        let result = arm(&mut store, NEW, || {
            stored_when_switching = Some(stored.get());
            Ok::<(), ()>(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(stored_when_switching, Some(on_trial(0, Some(NEW))));
        assert_eq!(store.load(), on_trial(0, Some(NEW)));
    }

    #[test]
    fn a_failed_switch_puts_the_previous_trial_back() {
        let previous = on_trial(2, Some(RUNNING));
        let mut store = Memory::holding(previous);
        let result = arm(&mut store, NEW, || Err("image doesn't check out"));
        assert_eq!(result, Err(ArmError::Switch("image doesn't check out")));
        assert_eq!(store.load(), previous);
    }

    #[test]
    fn a_failed_store_never_switches() {
        let mut store = Memory {
            failing: true,
            ..Memory::default()
        };
        let mut switched = false;
        let result = arm(&mut store, NEW, || {
            switched = true;
            Ok::<(), ()>(())
        });
        assert_eq!(result, Err(ArmError::Store("flash broken")));
        assert!(!switched);
    }

    #[test]
    fn ending_clears_the_trial_and_says_whether_there_was_one() {
        let mut store = Memory::holding(on_trial(1, Some(NEW)));
        assert_eq!(end(&mut store), Ok(true));
        assert_eq!(store.load(), Trial::default());
        assert_eq!(end(&mut store), Ok(false));
    }
}
