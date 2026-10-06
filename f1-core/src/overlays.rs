//! Short effects that play once over everything but a forced flag or colour:
//! start lights, chequered flag, flashes. They play one after another; at most
//! [`MAX_OVERLAYS`] wait, and more are dropped.

use std::{collections::VecDeque, time::Instant};

use crate::effect::Effect;

pub const MAX_OVERLAYS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Overlay {
    effect: Effect,
    started: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct Overlays {
    queue: VecDeque<Overlay>,
}

impl Overlays {
    pub fn push(&mut self, effect: Effect) -> bool {
        if self.queue.len() >= MAX_OVERLAYS || effect.duration().is_none() {
            return false;
        }
        self.queue.push_back(Overlay {
            effect,
            started: None,
        });
        true
    }

    pub fn tick(&mut self, now: Instant) {
        while let Some(front) = self.queue.front_mut() {
            let started = *front.started.get_or_insert(now);
            if !is_finished(front.effect, started, now) {
                break;
            }
            self.queue.pop_front();
        }
    }

    pub fn active(&self) -> Option<(Effect, Instant)> {
        let front = self.queue.front()?;
        front.started.map(|started| (front.effect, started))
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
        overlays.tick(t0);
        assert_eq!(overlays.active(), Some((FLASH, t0)));
    }

    #[test]
    fn overlay_stays_active_until_its_duration() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.tick(t0);
        overlays.tick(t0 + ms(599));
        assert_eq!(overlays.active(), Some((FLASH, t0)));
    }

    #[test]
    fn finished_overlay_is_removed() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.tick(t0);
        overlays.tick(t0 + ms(600));
        assert_eq!(overlays.active(), None);
        assert!(overlays.is_empty());
    }

    #[test]
    fn next_overlay_starts_when_previous_finishes() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.push(OTHER);
        overlays.tick(t0);
        overlays.tick(t0 + ms(600));
        assert_eq!(overlays.active(), Some((OTHER, t0 + ms(600))));
    }

    #[test]
    fn identical_overlays_play_one_after_another() {
        let t0 = Instant::now();
        let mut overlays = Overlays::default();
        overlays.push(FLASH);
        overlays.push(FLASH);
        overlays.tick(t0);
        overlays.tick(t0 + ms(600));
        assert_eq!(overlays.active(), Some((FLASH, t0 + ms(600))));
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
        overlays.tick(t0);
        assert_eq!(overlays.active(), Some((FLASH, t0)));
    }
}
