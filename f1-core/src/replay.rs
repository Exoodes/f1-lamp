//! Plays archived streams back as lamp inputs: the timeline gives messages in
//! order, the track state and the tracker turn them into race events, and the
//! playback clock says when each one is due.
//!
//! The replay owns the phase: before lights out it's PreSession, from lights
//! out to the last message Live, then PostSession. That works for races and
//! qualifying alike.

use std::{
    iter::Peekable,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    feed::SessionState,
    input::{Input, RaceEvent, SessionPhase},
    playback::Playback,
    stream::parse_line,
    timeline::{Stream, Timeline, TimelineError},
    track_state::{FeedMessage, TrackState},
    tracker::Tracker,
};

/// "Jump to lights out" lands this long before it, to see the start coming.
pub const LIGHTS_OUT_LEAD: Duration = Duration::from_secs(10);

/// What the replay wants next.
#[derive(Debug)]
pub enum Step {
    /// Send these now, then call `step` again.
    Send(Vec<Input>),
    /// Nothing is due for this long.
    Wait(Duration),
    Paused,
    /// The last message has been played.
    Finished,
    /// A line couldn't be read and was skipped. Call `step` again.
    Skipped(TimelineError),
}

/// What the web page can ask for, as JSON: `{"action":"play"}`,
/// `{"action":"speed","speed":60}`, `{"action":"seek","position_ms":4000000}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ReplayCommand {
    /// Plays; after the end, plays again from lights out.
    Play,
    Pause,
    Speed {
        speed: u32,
    },
    /// Jumps to just before lights out.
    LightsOut,
    Seek {
        position_ms: u64,
    },
}

/// For the web page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ReplayStatus {
    pub position_ms: u64,
    pub end_ms: u64,
    pub lights_out_ms: Option<u64>,
    pub speed: u32,
    pub playing: bool,
    pub finished: bool,
}

pub struct Replayer<'a> {
    files: Vec<(Stream, &'a str)>,
    timeline: Peekable<Timeline<'a>>,
    state: TrackState,
    tracker: Tracker,
    playback: Playback,
    /// First `Started`; `None` if the files never start.
    lights_out: Option<Duration>,
    /// Offset of the last message.
    end: Duration,
    /// Offset of the last message applied to `state`.
    applied: Option<Duration>,
    sent_phase: Option<SessionPhase>,
    /// After a jump: the position jumped to, whose phase and flag still
    /// have to be sent.
    sync_at: Option<Duration>,
    finished: bool,
}

impl<'a> Replayer<'a> {
    /// A paused replay at lights out minus [`LIGHTS_OUT_LEAD`], or at the
    /// start if there is no lights out.
    pub fn new(files: &[(Stream, &'a str)], speed: u32, now: Instant) -> Self {
        let (lights_out, end) = scan(files);
        let mut replayer = Replayer {
            files: files.to_vec(),
            timeline: Timeline::new(files).peekable(),
            state: TrackState::default(),
            tracker: Tracker::default(),
            playback: Playback::new(Duration::ZERO, speed),
            lights_out,
            end,
            applied: None,
            sent_phase: None,
            sync_at: None,
            finished: false,
        };
        replayer.seek(replayer.lights_out_start(), now);
        replayer
    }

    pub fn command(&mut self, command: ReplayCommand, now: Instant) {
        match command {
            ReplayCommand::Play => {
                if self.finished {
                    self.seek(self.lights_out_start(), now);
                }
                self.play(now);
            }
            ReplayCommand::Pause => self.pause(now),
            ReplayCommand::Speed { speed } => self.set_speed(speed, now),
            ReplayCommand::LightsOut => self.seek(self.lights_out_start(), now),
            ReplayCommand::Seek { position_ms } => {
                let to = Duration::from_millis(position_ms).min(self.end);
                self.seek(to, now);
            }
        }
    }

    pub fn play(&mut self, now: Instant) {
        self.playback.play(now);
    }

    pub fn pause(&mut self, now: Instant) {
        self.playback.pause(now);
    }

    pub fn set_speed(&mut self, speed: u32, now: Instant) {
        self.playback.set_speed(speed, now);
    }

    /// Where "jump to lights out" lands.
    pub fn lights_out_start(&self) -> Duration {
        self.lights_out
            .map_or(Duration::ZERO, |t| t.saturating_sub(LIGHTS_OUT_LEAD))
    }

    /// Jumps to `to`, playing or paused as before. Everything before `to` is
    /// applied silently; the next `step` sends the phase and flag at `to`.
    pub fn seek(&mut self, to: Duration, now: Instant) {
        // Messages at or after `to` already applied: start over.
        if self.applied.is_some_and(|t| t >= to) {
            self.timeline = Timeline::new(&self.files).peekable();
            self.state = TrackState::default();
            self.tracker = Tracker::default();
            self.applied = None;
        }
        while let Some(item) = self.timeline.next_if(|item| match item {
            Ok((offset, _)) => *offset < to,
            Err(_) => true,
        }) {
            if let Ok((offset, msg)) = item {
                self.tracker.apply(&msg);
                self.state.apply(msg);
                self.applied = Some(offset);
            }
        }
        self.playback.seek(to, now);
        self.sync_at = Some(to);
        self.finished = false;
    }

    pub fn status(&self, now: Instant) -> ReplayStatus {
        let position = self.playback.position(now).min(self.end);
        ReplayStatus {
            position_ms: millis(position),
            end_ms: millis(self.end),
            lights_out_ms: self.lights_out.map(millis),
            speed: self.playback.speed(),
            playing: self.playback.is_playing(),
            finished: self.finished,
        }
    }

    pub fn step(&mut self, now: Instant) -> Step {
        let mut out = Vec::new();

        if let Some(position) = self.sync_at.take() {
            self.send_phase(self.phase_at(position), &mut out);
            // Also when there's none: a jump back from a flag to before the
            // start, or between qualifying parts, must not leave it showing.
            let flag = self.state.flag();
            out.push(race(
                flag.map_or(RaceEvent::FlagCleared, RaceEvent::TrackFlag),
                now,
            ));
        }

        loop {
            let offset = match self.timeline.peek() {
                None => {
                    if !self.finished {
                        self.finished = true;
                        self.send_phase(SessionPhase::PostSession, &mut out);
                    }
                    return if out.is_empty() {
                        Step::Finished
                    } else {
                        Step::Send(out)
                    };
                }
                Some(Err(_)) => {
                    if !out.is_empty() {
                        return Step::Send(out);
                    }
                    if let Some(Err(e)) = self.timeline.next() {
                        return Step::Skipped(e);
                    }
                    continue;
                }
                Some(Ok((offset, _))) => *offset,
            };

            match self.playback.wait_for(offset, now) {
                Some(wait) if wait.is_zero() => {}
                not_due => {
                    if !out.is_empty() {
                        return Step::Send(out);
                    }
                    return not_due.map_or(Step::Paused, Step::Wait);
                }
            }

            if let Some(Ok((offset, msg))) = self.timeline.next() {
                self.send_phase(self.phase_at(offset), &mut out);
                for event in self.tracker.apply(&msg) {
                    out.push(race(event, now));
                }
                for event in self.state.apply(msg) {
                    out.push(race(event, now));
                }
                self.applied = Some(offset);
            }
        }
    }

    fn phase_at(&self, position: Duration) -> SessionPhase {
        match self.lights_out {
            _ if self.finished || position > self.end => SessionPhase::PostSession,
            Some(lights_out) if position < lights_out => SessionPhase::PreSession,
            _ => SessionPhase::Live,
        }
    }

    fn send_phase(&mut self, phase: SessionPhase, out: &mut Vec<Input>) {
        if self.sent_phase != Some(phase) {
            self.sent_phase = Some(phase);
            out.push(Input::Phase(phase));
        }
    }
}

/// Lights out and the last offset. Cheap: only SessionStatus is parsed, and
/// the end is the offset of each file's last line.
fn scan(files: &[(Stream, &str)]) -> (Option<Duration>, Duration) {
    let sessions: Vec<(Stream, &str)> = files
        .iter()
        .copied()
        .filter(|(stream, _)| *stream == Stream::SessionStatus)
        .collect();
    let lights_out = Timeline::new(&sessions)
        .flatten()
        .find_map(|(offset, msg)| match msg {
            FeedMessage::Session(s) if s.status == Some(SessionState::Started) => Some(offset),
            _ => None,
        });
    let end = files
        .iter()
        .filter_map(|(_, text)| {
            let last = text.lines().rev().find(|l| !l.trim().is_empty())?;
            parse_line(last).ok().map(|line| line.offset)
        })
        .max()
        .unwrap_or(Duration::ZERO);
    (lights_out, end)
}

fn race(event: RaceEvent, now: Instant) -> Input {
    Input::Race {
        event,
        received: now,
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::TrackFlag;

    const TRACK: &str = "\
00:00:01.000{\"Status\":\"1\"}
00:01:00.000{\"Status\":\"2\"}
00:01:30.000{\"Status\":\"1\"}";
    const SESSION: &str = "\
00:00:00.500{\"Status\":\"Inactive\"}
00:00:40.000{\"Status\":\"Started\"}
00:02:00.000{\"Status\":\"Finished\"}";

    fn files() -> Vec<(Stream, &'static str)> {
        vec![
            (Stream::TrackStatus, TRACK),
            (Stream::SessionStatus, SESSION),
        ]
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn race_events(inputs: &[Input]) -> Vec<RaceEvent> {
        inputs
            .iter()
            .filter_map(|i| match i {
                Input::Race { event, .. } => Some(*event),
                _ => None,
            })
            .collect()
    }

    fn phases(inputs: &[Input]) -> Vec<SessionPhase> {
        inputs
            .iter()
            .filter_map(|i| match i {
                Input::Phase(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    /// Steps until the replay waits, pauses or finishes; returns all inputs.
    fn drain(r: &mut Replayer, now: Instant) -> (Vec<Input>, Step) {
        let mut all = Vec::new();
        loop {
            match r.step(now) {
                Step::Send(inputs) => all.extend(inputs),
                Step::Skipped(e) => panic!("{e}"),
                other => return (all, other),
            }
        }
    }

    #[test]
    fn new_replay_is_paused_ten_seconds_before_lights_out() {
        let t0 = Instant::now();
        let r = Replayer::new(&files(), 10, t0);
        let status = r.status(t0);
        assert_eq!(status.position_ms, 30_000);
        assert_eq!(status.lights_out_ms, Some(40_000));
        assert_eq!(status.end_ms, 120_000);
        assert!(!status.playing);
    }

    #[test]
    fn first_step_sends_pre_session_and_clears_the_flag() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        let (inputs, step) = drain(&mut r, t0);
        assert_eq!(
            inputs,
            [
                Input::Phase(SessionPhase::PreSession),
                Input::Race {
                    event: RaceEvent::FlagCleared,
                    received: t0
                }
            ]
        );
        assert!(matches!(step, Step::Paused));
    }

    #[test]
    fn playing_waits_for_the_next_message_at_speed() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.play(t0);
        let (_, step) = drain(&mut r, t0);
        // Lights out at 40 s, position 30 s, 10x: one second.
        assert!(matches!(step, Step::Wait(w) if w == secs(1)));
    }

    #[test]
    fn lights_out_goes_live_with_start_lights_and_green() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        drain(&mut r, t0);
        r.play(t0);
        let (inputs, _) = drain(&mut r, t0 + secs(1));
        assert_eq!(phases(&inputs), [SessionPhase::Live]);
        assert_eq!(
            race_events(&inputs),
            [
                RaceEvent::StartLights,
                RaceEvent::TrackFlag(TrackFlag::Green)
            ]
        );
        // The phase comes first, so the flag shows straight away.
        assert_eq!(inputs[0], Input::Phase(SessionPhase::Live));
    }

    #[test]
    fn whole_session_plays_then_post_session_and_finished() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.play(t0);
        let (inputs, step) = drain(&mut r, t0 + secs(60));
        assert_eq!(
            race_events(&inputs),
            [
                // The first sync: no flag before the start.
                RaceEvent::FlagCleared,
                RaceEvent::StartLights,
                RaceEvent::TrackFlag(TrackFlag::Green),
                RaceEvent::TrackFlag(TrackFlag::Yellow),
                RaceEvent::TrackFlag(TrackFlag::Green),
                // The session finished.
                RaceEvent::FlagCleared,
            ]
        );
        assert_eq!(
            phases(&inputs),
            [
                SessionPhase::PreSession,
                SessionPhase::Live,
                SessionPhase::PostSession
            ]
        );
        assert!(matches!(step, Step::Finished));
        assert!(r.status(t0 + secs(60)).finished);
    }

    #[test]
    fn paused_replay_sends_nothing_however_long_it_waits() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        drain(&mut r, t0);
        let (inputs, step) = drain(&mut r, t0 + secs(3600));
        assert!(inputs.is_empty());
        assert!(matches!(step, Step::Paused));
    }

    #[test]
    fn jump_into_a_yellow_sends_live_and_yellow_at_once() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        drain(&mut r, t0);
        r.seek(secs(70), t0);
        let (inputs, _) = drain(&mut r, t0);
        assert_eq!(phases(&inputs), [SessionPhase::Live]);
        // No start lights: they belong to the moment that was skipped.
        assert_eq!(
            race_events(&inputs),
            [RaceEvent::TrackFlag(TrackFlag::Yellow)]
        );
    }

    #[test]
    fn jump_back_before_lights_out_plays_the_start_again() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.play(t0);
        drain(&mut r, t0 + secs(60));
        r.seek(r.lights_out_start(), t0 + secs(60));
        let (inputs, _) = drain(&mut r, t0 + secs(61));
        assert_eq!(
            phases(&inputs),
            [SessionPhase::PreSession, SessionPhase::Live]
        );
        assert_eq!(
            race_events(&inputs),
            [
                // The jump leaves the finished session's state behind.
                RaceEvent::FlagCleared,
                RaceEvent::StartLights,
                RaceEvent::TrackFlag(TrackFlag::Green)
            ]
        );
        assert!(!r.status(t0 + secs(61)).finished);
    }

    #[test]
    fn speed_change_mid_wait_shortens_the_wait() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 1, t0);
        r.play(t0);
        drain(&mut r, t0);
        r.set_speed(10, t0);
        let (_, step) = drain(&mut r, t0);
        assert!(matches!(step, Step::Wait(w) if w == secs(1)));
    }

    #[test]
    fn files_without_a_start_play_live_from_the_beginning() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&[(Stream::TrackStatus, TRACK)], 10, t0);
        assert_eq!(r.status(t0).position_ms, 0);
        let (inputs, _) = drain(&mut r, t0);
        assert_eq!(phases(&inputs), [SessionPhase::Live]);
    }

    #[test]
    fn bad_line_is_skipped_and_reported() {
        let t0 = Instant::now();
        let track = "00:00:01.000{\"Status\":\"1\"}\nbroken\n00:00:02.000{\"Status\":\"2\"}";
        let mut r = Replayer::new(&[(Stream::TrackStatus, track)], 10, t0);
        r.play(t0);
        let mut skipped = 0;
        loop {
            match r.step(t0 + secs(10)) {
                Step::Skipped(e) => {
                    assert_eq!(e.line, 2);
                    skipped += 1;
                }
                Step::Send(_) => {}
                _ => break,
            }
        }
        assert_eq!(skipped, 1);
    }

    #[test]
    fn commands_parse_from_json() {
        let cases = [
            (r#"{"action":"play"}"#, ReplayCommand::Play),
            (r#"{"action":"pause"}"#, ReplayCommand::Pause),
            (
                r#"{"action":"speed","speed":60}"#,
                ReplayCommand::Speed { speed: 60 },
            ),
            (r#"{"action":"lights_out"}"#, ReplayCommand::LightsOut),
            (
                r#"{"action":"seek","position_ms":4000}"#,
                ReplayCommand::Seek { position_ms: 4000 },
            ),
        ];
        for (json, expected) in cases {
            assert_eq!(
                serde_json::from_str::<ReplayCommand>(json).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert!(serde_json::from_str::<ReplayCommand>(r#"{"action":"rewind"}"#).is_err());
    }

    #[test]
    fn play_and_pause_commands_control_the_clock() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.command(ReplayCommand::Play, t0);
        assert!(r.status(t0).playing);
        r.command(ReplayCommand::Pause, t0 + secs(1));
        let status = r.status(t0 + secs(5));
        assert!(!status.playing);
        assert_eq!(status.position_ms, 40_000);
    }

    #[test]
    fn speed_command_changes_speed() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.command(ReplayCommand::Speed { speed: 1 }, t0);
        assert_eq!(r.status(t0).speed, 1);
    }

    #[test]
    fn play_after_the_end_starts_again_from_lights_out() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.play(t0);
        drain(&mut r, t0 + secs(60));
        r.command(ReplayCommand::Play, t0 + secs(60));
        let status = r.status(t0 + secs(60));
        assert!(status.playing && !status.finished);
        assert_eq!(status.position_ms, 30_000);
    }

    #[test]
    fn lights_out_command_jumps_before_the_start() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.command(
            ReplayCommand::Seek {
                position_ms: 90_000,
            },
            t0,
        );
        r.command(ReplayCommand::LightsOut, t0);
        assert_eq!(r.status(t0).position_ms, 30_000);
    }

    #[test]
    fn seek_past_the_end_stops_at_the_end() {
        let t0 = Instant::now();
        let mut r = Replayer::new(&files(), 10, t0);
        r.command(
            ReplayCommand::Seek {
                position_ms: u64::MAX,
            },
            t0,
        );
        assert_eq!(r.status(t0).position_ms, 120_000);
    }
}
