//! Short effects that play once over everything but a forced flag or colour:
//! start lights, chequered flag, flashes. They play one after another; at most
//! [`MAX_OVERLAYS`] wait, and more are dropped. Each is numbered as it starts,
//! and the last [`RECENT_FLASHES`] shown are kept for the web page, which
//! asks only every 2 s and would miss most of them.

use std::{collections::VecDeque, time::Instant};

use crate::effect::Effect;

/// Room for every knockout of a qualifying part, which all come in one
/// TimingData message (Monza 2026), behind the part's chequered flag.
pub const MAX_OVERLAYS: usize = 8;
/// Of 22 cars, 6 are knocked out after Q1 and 6 after Q2, plus the
/// chequered flag.
const MOST_AT_ONCE: usize = 6 + 1;
const _: () = assert!(MAX_OVERLAYS >= MOST_AT_ONCE);

/// Overlays kept after they played: enough for those of one poll of the
/// page (a flash lasts 0.3-2 s, a fade 3-6 s).
pub const RECENT_FLASHES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Overlay {
    effect: Effect,
    started: Option<Instant>,
    /// Given when it starts.
    seq: u32,
}

/// An overlay that has played to the end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Played {
    /// Overlays are numbered in the order they start.
    pub seq: u32,
    pub effect: Effect,
    pub ended: Instant,
}

/// The last [`RECENT_FLASHES`] overlays the lamp showed, oldest first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Recent([Option<Played>; RECENT_FLASHES]);

impl Recent {
    pub fn push(&mut self, played: Played) {
        self.0.rotate_left(1);
        self.0[RECENT_FLASHES - 1] = Some(played);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Played> {
        self.0.iter().flatten()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Overlays {
    queue: VecDeque<Overlay>,
    next_seq: u32,
}

impl Overlays {
    pub fn push(&mut self, effect: Effect) -> bool {
        if self.queue.len() >= MAX_OVERLAYS || effect.duration().is_none() {
            return false;
        }
        self.queue.push_back(Overlay {
            effect,
            started: None,
            seq: 0,
        });
        true
    }

    /// Starts the next overlay when the one before is over; `ended` hears of
    /// each that played to the end.
    pub fn tick(&mut self, now: Instant, mut ended: impl FnMut(Played)) {
        while let Some(front) = self.queue.front_mut() {
            let started = match front.started {
                Some(started) => started,
                None => {
                    front.started = Some(now);
                    front.seq = self.next_seq;
                    self.next_seq = self.next_seq.wrapping_add(1);
                    now
                }
            };
            if !is_finished(front.effect, started, now) {
                break;
            }
            ended(Played {
                seq: front.seq,
                effect: front.effect,
                ended: started + front.effect.duration().unwrap_or_default(),
            });
            self.queue.pop_front();
        }
    }

    /// The overlay showing: its effect, when it started and its number.
    pub fn active(&self) -> Option<(Effect, Instant, u32)> {
        let front = self.queue.front()?;
        front
            .started
            .map(|started| (front.effect, started, front.seq))
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

fn is_finished(effect: Effect, started: Instant, now: Instant) -> bool {
    effect
        .duration()
        .is_some_and(|length| now.duration_since(started) >= length)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::color::Rgb;

    // 2 flashes × 300 ms = 600 ms.
    const FLASH: Effect = Effect::flash(Rgb::RED, 2);
    const OTHER: Effect = Effect::flash(Rgb::BLUE, 2);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn new_queue_has_nothing_active() {
        assert_eq!(Overlays::default().active(), None);
    }

    #[test]
    fn pushed_overlay_is_not_active_until_ticked() {
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        assert_eq!(overlays.active(), None);
    }

    #[test]
    fn first_tick_starts_the_front_overlay() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.tick(t0, |_| {});
        assert_eq!(overlays.active(), Some((FLASH, t0, 0)));
    }

    #[test]
    fn overlay_stays_active_until_its_duration() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.tick(t0, |_| {});
        overlays.tick(t0 + ms(599), |_| {});
        assert_eq!(overlays.active(), Some((FLASH, t0, 0)));
    }

    #[test]
    fn finished_overlay_is_removed() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.tick(t0, |_| {});
        overlays.tick(t0 + ms(600), |_| {});
        assert_eq!(overlays.active(), None);
        assert!(overlays.is_empty());
    }

    #[test]
    fn next_overlay_starts_when_previous_finishes() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.push(OTHER);
        overlays.tick(t0, |_| {});
        overlays.tick(t0 + ms(600), |_| {});
        assert_eq!(overlays.active(), Some((OTHER, t0 + ms(600), 1)));
    }

    #[test]
    fn identical_overlays_play_one_after_another() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.push(FLASH);
        overlays.tick(t0, |_| {});
        overlays.tick(t0 + ms(600), |_| {});
        assert_eq!(overlays.active(), Some((FLASH, t0 + ms(600), 1)));
        assert_eq!(overlays.len(), 1);
    }

    #[test]
    fn queue_is_capped_at_max_overlays() {
        let mut overlays = Overlays::default();
        for _ in 0..MAX_OVERLAYS {
            assert!(overlays.push(FLASH));
        }
        assert!(!overlays.push(FLASH));
        assert_eq!(overlays.len(), MAX_OVERLAYS);
    }

    #[test]
    fn endless_effect_is_rejected() {
        let mut overlays = Overlays::default();
        assert!(!overlays.push(Effect::Solid(Rgb::RED)));
        assert!(overlays.is_empty());
    }

    #[test]
    fn zero_length_overlay_is_skipped_in_the_same_tick() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(Effect::flash(Rgb::RED, 0));
        overlays.push(FLASH);
        overlays.tick(t0, |_| {});
        assert_eq!(overlays.active(), Some((FLASH, t0, 1)));
    }

    #[test]
    fn an_overlay_that_played_is_reported_with_its_number_and_end() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.push(OTHER);
        let mut played = Vec::new();
        overlays.tick(t0, |p| played.push(p));
        assert!(played.is_empty());
        // Late: the end is when it was due, not this tick.
        overlays.tick(t0 + ms(650), |p| played.push(p));
        assert_eq!(
            played,
            [Played {
                seq: 0,
                effect: FLASH,
                ended: t0 + ms(600)
            }]
        );
    }

    #[test]
    fn recent_keeps_the_last_four_oldest_first() {
        let t0 = Instant::now();
        let mut recent = Recent::default();
        for seq in 0..6 {
            recent.push(Played {
                seq,
                effect: FLASH,
                ended: t0,
            });
        }
        let seqs: Vec<u32> = recent.iter().map(|p| p.seq).collect();
        assert_eq!(seqs, [2, 3, 4, 5]);
    }
}
