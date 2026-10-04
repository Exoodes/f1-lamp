//! The replay's clock: where in the session it is, how fast it runs, and
//! whether it's paused. Positions are offsets from the start of the feed, the
//! same as in the archive files.

use std::time::{Duration, Instant};

/// Slowest speed: real time, for watching a recording along with the lamp.
pub const MIN_SPEED: u32 = 1;
/// Fastest speed: a whole race in about a minute.
pub const MAX_SPEED: u32 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playback {
    /// Position when `since` was set, or the position while paused.
    position: Duration,
    /// When playing: the moment `position` was taken.
    since: Option<Instant>,
    speed: u32,
}

impl Playback {
    /// A paused clock at `position`. The speed is clamped to the allowed range.
    pub fn new(position: Duration, speed: u32) -> Self {
        Playback {
            position,
            since: None,
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
        }
    }

    pub fn position(&self, now: Instant) -> Duration {
        match self.since {
            Some(since) => self.position + now.saturating_duration_since(since) * self.speed,
            None => self.position,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.since.is_some()
    }

    pub fn speed(&self) -> u32 {
        self.speed
    }

    pub fn play(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
        }
    }

    pub fn pause(&mut self, now: Instant) {
        self.position = self.position(now);
        self.since = None;
    }

    /// Changes speed without jumping: the position so far is kept.
    pub fn set_speed(&mut self, speed: u32, now: Instant) {
        self.rebase(now);
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Moves to `position`, playing or paused as before.
    pub fn seek(&mut self, position: Duration, now: Instant) {
        self.position = position;
        if self.since.is_some() {
            self.since = Some(now);
        }
    }

    /// How long until the clock reaches `offset`: zero if it already has,
    /// `None` while paused.
    pub fn wait_for(&self, offset: Duration, now: Instant) -> Option<Duration> {
        self.since?;
        Some(offset.saturating_sub(self.position(now)) / self.speed)
    }

    /// Folds the time played so far into `position`.
    fn rebase(&mut self, now: Instant) {
        if self.since.is_some() {
            self.position = self.position(now);
            self.since = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn new_clock_is_paused_at_its_position() {
        let t0 = Instant::now();
        let p = Playback::new(secs(100), 10);
        assert!(!p.is_playing());
        assert_eq!(p.position(t0 + secs(5)), secs(100));
    }

    #[test]
    fn playing_moves_speed_times_faster_than_real_time() {
        let t0 = Instant::now();
        let mut p = Playback::new(secs(100), 60);
        p.play(t0);
        assert_eq!(p.position(t0 + secs(2)), secs(220));
    }

    #[test]
    fn real_time_speed_moves_one_to_one() {
        let t0 = Instant::now();
        let mut p = Playback::new(Duration::ZERO, 1);
        p.play(t0);
        assert_eq!(p.position(t0 + secs(7)), secs(7));
    }

    #[test]
    fn pause_keeps_the_position_reached() {
        let t0 = Instant::now();
        let mut p = Playback::new(Duration::ZERO, 10);
        p.play(t0);
        p.pause(t0 + secs(3));
        assert_eq!(p.position(t0 + secs(100)), secs(30));
    }

    #[test]
    fn play_twice_doesnt_restart_the_clock() {
        let t0 = Instant::now();
        let mut p = Playback::new(Duration::ZERO, 10);
        p.play(t0);
        p.play(t0 + secs(3));
        assert_eq!(p.position(t0 + secs(4)), secs(40));
    }

    #[test]
    fn speed_change_keeps_the_position_so_far() {
        let t0 = Instant::now();
        let mut p = Playback::new(Duration::ZERO, 10);
        p.play(t0);
        p.set_speed(60, t0 + secs(2)); // 20 s played at 10x
        assert_eq!(p.position(t0 + secs(3)), secs(80)); // + 1 s at 60x
    }

    #[test]
    fn speed_is_clamped() {
        assert_eq!(Playback::new(Duration::ZERO, 0).speed(), MIN_SPEED);
        assert_eq!(Playback::new(Duration::ZERO, 100_000).speed(), MAX_SPEED);
        let mut p = Playback::new(Duration::ZERO, 10);
        p.set_speed(0, Instant::now());
        assert_eq!(p.speed(), MIN_SPEED);
    }

    #[test]
    fn seek_while_playing_keeps_playing_from_there() {
        let t0 = Instant::now();
        let mut p = Playback::new(Duration::ZERO, 10);
        p.play(t0);
        p.seek(secs(500), t0 + secs(5));
        assert_eq!(p.position(t0 + secs(6)), secs(510));
    }

    #[test]
    fn seek_while_paused_stays_paused() {
        let t0 = Instant::now();
        let mut p = Playback::new(Duration::ZERO, 10);
        p.seek(secs(500), t0);
        assert!(!p.is_playing());
        assert_eq!(p.position(t0 + secs(9)), secs(500));
    }

    #[test]
    fn wait_is_the_feed_gap_divided_by_speed() {
        let t0 = Instant::now();
        let mut p = Playback::new(secs(100), 60);
        p.play(t0);
        assert_eq!(p.wait_for(secs(160), t0), Some(secs(1)));
    }

    #[test]
    fn wait_for_a_past_offset_is_zero() {
        let t0 = Instant::now();
        let mut p = Playback::new(secs(100), 60);
        p.play(t0);
        assert_eq!(p.wait_for(secs(50), t0), Some(Duration::ZERO));
    }

    #[test]
    fn wait_while_paused_is_none() {
        let p = Playback::new(secs(100), 60);
        assert_eq!(p.wait_for(secs(160), Instant::now()), None);
    }
}
