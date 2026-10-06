//! How smoothly the render loop runs: frames per period and the longest gap
//! between two of them. A frame comes every 20 ms or so; a gap of 100 ms or
//! more is a visible jerk in a moving effect (a chase, the start lights),
//! e.g. when other tasks keep the CPU busy (TLS handshakes) or a log line
//! waits for the serial port.

use std::time::{Duration, Instant};

/// How often the figures are reported.
pub const REPORT_EVERY: Duration = Duration::from_secs(60);

/// The figures for one period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub frames: u32,
    pub longest_gap: Duration,
    /// How long the period was.
    pub over: Duration,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameStats {
    since: Instant,
    last: Instant,
    frames: u32,
    longest_gap: Duration,
}

impl FrameStats {
    pub fn new(now: Instant) -> Self {
        FrameStats {
            since: now,
            last: now,
            frames: 0,
            longest_gap: Duration::ZERO,
        }
    }

    /// A frame was rendered at `now`. Every `REPORT_EVERY`, the figures for
    /// the period that just ended; then a new one begins.
    pub fn frame(&mut self, now: Instant) -> Option<Report> {
        self.longest_gap = self
            .longest_gap
            .max(now.saturating_duration_since(self.last));
        self.last = now;
        self.frames += 1;
        let over = now.saturating_duration_since(self.since);
        if over < REPORT_EVERY {
            return None;
        }
        let report = Report {
            frames: self.frames,
            longest_gap: self.longest_gap,
            over,
        };
        self.since = now;
        self.frames = 0;
        self.longest_gap = Duration::ZERO;
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Frames every `step` from `t0` until (not including) `until`.
    fn run(stats: &mut FrameStats, t0: Instant, step: Duration, until: Duration) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut at = step;
        while at < until {
            reports.extend(stats.frame(t0 + at));
            at += step;
        }
        reports
    }

    #[test]
    fn nothing_is_reported_before_the_period_ends() {
        let t0 = Instant::now();
        let mut s = FrameStats::new(t0);
        assert!(run(&mut s, t0, ms(20), REPORT_EVERY).is_empty());
    }

    #[test]
    fn a_steady_period_reports_its_frames_and_gap() {
        let t0 = Instant::now();
        let mut s = FrameStats::new(t0);
        let reports = run(&mut s, t0, ms(20), REPORT_EVERY + ms(20));
        assert_eq!(
            reports,
            [Report {
                frames: 3000,
                longest_gap: ms(20),
                over: REPORT_EVERY
            }]
        );
    }

    #[test]
    fn one_late_frame_is_the_longest_gap() {
        let t0 = Instant::now();
        let mut s = FrameStats::new(t0);
        s.frame(t0 + ms(20));
        s.frame(t0 + ms(320)); // 300 ms late
        let report = run(&mut s, t0 + ms(320), ms(20), REPORT_EVERY)
            .pop()
            .unwrap();
        assert_eq!(report.longest_gap, ms(300));
    }

    #[test]
    fn each_period_starts_afresh() {
        let t0 = Instant::now();
        let mut s = FrameStats::new(t0);
        s.frame(t0 + ms(500));
        let first = s.frame(t0 + REPORT_EVERY).unwrap();
        let second = run(&mut s, t0 + REPORT_EVERY, ms(20), REPORT_EVERY + ms(20))
            .pop()
            .unwrap();
        assert!(first.longest_gap > ms(500));
        assert_eq!(second.longest_gap, ms(20));
    }
}
