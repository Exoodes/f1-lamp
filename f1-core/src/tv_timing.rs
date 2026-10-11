//! [`TvTiming`]: the session clock and the lap counter as the TV shows them,
//! for setting the TV delay by eye: the page shows them, and with the right
//! delay they change together with the TV's.
//!
//! Readings are kept with the moment they were valid, and the TV's view is
//! picked at the time asked, with the delay set then: a changed delay shows
//! at once, without waiting for the next reading (the clock only sends one
//! when it starts or stops).

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::input::TimingInfo;

/// Readings kept of each kind: laps take a minute or more, the clock only
/// starts and stops, and the TV is at most `MAX_TV_DELAY_MS` behind.
const KEPT: usize = 8;

/// A clock reading whose own time is further than this from its arrival
/// came with the state on connecting (it can be minutes old) and is placed
/// at its own time. Closer ones are placed at their arrival, like race
/// events, so the clock lines up with the TV exactly when the events do.
pub const FRESH: Duration = Duration::from_secs(10);

/// When a reading was valid: at `arrival`, unless its own time (`utc_ms`,
/// Unix ms) is more than [`FRESH`] away from the arrival's (`arrival_ms`).
pub fn valid_at(arrival: Instant, arrival_ms: i64, utc_ms: Option<i64>) -> Instant {
    let Some(utc_ms) = utc_ms else {
        return arrival;
    };
    let age = arrival_ms - utc_ms;
    if age.unsigned_abs() <= FRESH.as_millis() as u64 {
        return arrival;
    }
    let offset = Duration::from_millis(age.unsigned_abs());
    if age > 0 {
        arrival.checked_sub(offset).unwrap_or(arrival)
    } else {
        arrival + offset
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClockReading {
    at: Instant,
    remaining_ms: u32,
    running: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LapReading {
    at: Instant,
    current: Option<u32>,
    total: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct TvTiming {
    clocks: VecDeque<ClockReading>,
    laps: VecDeque<LapReading>,
}

/// The clock as the TV shows it: `remaining_ms` at `anchor`, counting down
/// from there while `running`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TvClock {
    pub anchor: Instant,
    pub remaining_ms: u32,
    pub running: bool,
}

impl TvClock {
    pub fn remaining_at(&self, now: Instant) -> u32 {
        if !self.running {
            return self.remaining_ms;
        }
        let gone = now.saturating_duration_since(self.anchor).as_millis();
        u32::try_from(u128::from(self.remaining_ms).saturating_sub(gone)).unwrap_or(0)
    }
}

/// What the TV shows at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TvView {
    pub clock: Option<TvClock>,
    pub lap: Option<u32>,
    pub total_laps: Option<u32>,
}

impl TvTiming {
    /// A reading valid at `at`.
    pub fn add(&mut self, info: TimingInfo, at: Instant) {
        match info {
            TimingInfo::Clock {
                remaining_ms,
                running,
                ..
            } => push(
                &mut self.clocks,
                ClockReading {
                    at,
                    remaining_ms,
                    running,
                },
            ),
            TimingInfo::Lap { current, total } => {
                // A message carries only what changed.
                let last = self.laps.back();
                let reading = LapReading {
                    at,
                    current: current.or(last.and_then(|l| l.current)),
                    total: total.or(last.and_then(|l| l.total)),
                };
                push(&mut self.laps, reading);
            }
        }
    }

    /// A new session starts from nothing.
    pub fn clear(&mut self) {
        self.clocks.clear();
        self.laps.clear();
    }

    /// What the TV shows at `now`, `delay` behind the feed.
    pub fn view(&self, now: Instant, delay: Duration) -> TvView {
        let Some(on_tv) = now.checked_sub(delay) else {
            return TvView::default();
        };
        let clock = self
            .clocks
            .iter()
            .rev()
            .find(|c| c.at <= on_tv)
            .map(|c| TvClock {
                anchor: c.at + delay,
                remaining_ms: c.remaining_ms,
                running: c.running,
            });
        let lap = self.laps.iter().rev().find(|l| l.at <= on_tv);
        TvView {
            clock,
            lap: lap.and_then(|l| l.current),
            total_laps: lap.and_then(|l| l.total),
        }
    }
}

fn push<T>(readings: &mut VecDeque<T>, reading: T) {
    if readings.len() == KEPT {
        readings.pop_front();
    }
    readings.push_back(reading);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn clock(remaining_secs: u32, running: bool) -> TimingInfo {
        TimingInfo::Clock {
            remaining_ms: remaining_secs * 1000,
            running,
            utc_ms: None,
        }
    }

    fn lap(current: Option<u32>, total: Option<u32>) -> TimingInfo {
        TimingInfo::Lap { current, total }
    }

    #[test]
    fn the_tv_clock_starts_one_delay_after_the_feed_clock() {
        let t0 = Instant::now();
        let mut tv = TvTiming::default();
        tv.add(clock(18 * 60, true), t0);
        let delay = s(44);
        assert_eq!(tv.view(t0 + s(43), delay).clock, None);
        let shown = tv.view(t0 + s(44), delay).clock.unwrap();
        assert_eq!(shown.anchor, t0 + s(44));
        assert_eq!(shown.remaining_at(t0 + s(44)), 18 * 60 * 1000);
        assert_eq!(shown.remaining_at(t0 + s(104)), 17 * 60 * 1000);
    }

    #[test]
    fn a_stopped_clock_stands_still() {
        let t0 = Instant::now();
        let mut tv = TvTiming::default();
        tv.add(clock(600, true), t0);
        tv.add(clock(540, false), t0 + s(60));
        let shown = tv.view(t0 + s(100), s(10)).clock.unwrap();
        assert!(!shown.running);
        assert_eq!(shown.remaining_at(t0 + s(500)), 540_000);
    }

    #[test]
    fn a_changed_delay_shows_at_once() {
        let t0 = Instant::now();
        let mut tv = TvTiming::default();
        tv.add(clock(600, true), t0);
        let now = t0 + s(100);
        let at = |delay| tv.view(now, s(delay)).clock.unwrap().remaining_at(now);
        assert_eq!(at(40), 540_000);
        assert_eq!(at(41), 541_000);
    }

    #[test]
    fn the_tv_lap_changes_one_delay_after_the_feed_lap() {
        let t0 = Instant::now();
        let mut tv = TvTiming::default();
        tv.add(lap(Some(1), Some(62)), t0);
        tv.add(lap(Some(2), None), t0 + s(100));
        let delay = s(44);
        let shown = |at| tv.view(at, delay);
        assert_eq!(shown(t0 + s(143)).lap, Some(1));
        assert_eq!(shown(t0 + s(144)).lap, Some(2));
        // The total carries over from the first message.
        assert_eq!(shown(t0 + s(144)).total_laps, Some(62));
    }

    #[test]
    fn only_the_last_readings_are_kept() {
        let t0 = Instant::now();
        let mut tv = TvTiming::default();
        for n in 0..20 {
            tv.add(lap(Some(n), Some(62)), t0 + s(u64::from(n) * 60));
        }
        assert_eq!(tv.laps.len(), KEPT);
        assert_eq!(tv.view(t0 + s(19 * 60), Duration::ZERO).lap, Some(19));
    }

    #[test]
    fn clearing_forgets_the_session() {
        let t0 = Instant::now();
        let mut tv = TvTiming::default();
        tv.add(clock(600, true), t0);
        tv.add(lap(Some(3), Some(62)), t0);
        tv.clear();
        assert_eq!(tv.view(t0 + s(1), Duration::ZERO), TvView::default());
    }

    #[test]
    fn a_fresh_reading_counts_from_its_arrival() {
        let arrival = Instant::now() + s(1000);
        assert_eq!(valid_at(arrival, 50_000, Some(49_200)), arrival);
        assert_eq!(valid_at(arrival, 50_000, None), arrival);
    }

    #[test]
    fn an_old_reading_from_connecting_counts_from_its_own_time() {
        let arrival = Instant::now() + s(1000);
        assert_eq!(valid_at(arrival, 200_000, Some(20_000)), arrival - s(180));
    }
}
