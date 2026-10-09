//! With `--features replay` or `showcase`: plays a session on the lamp, through
//! the same parsers, track-state machine and tracker the live feed will use.
//!
//! The session comes from `[replay] dir` in cfg.toml, or is the hand-written
//! showcase race (see build.rs), and is embedded in the firmware. The replay owns the phase, so the scheduler
//! doesn't send one while this runs. The web page controls it through
//! [`handle`]: play, pause, speed, jump.

use std::{
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
        Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use f1_core::{
    heartbeat::Heartbeat,
    input::Input,
    replay::{ReplayCommand, ReplayStatus, Replayer, Step},
    timeline::Stream,
};

/// Speed until the page sets another. The showcase is written for real time;
/// at 60x a real race takes about two minutes.
const SPEED: u32 = if cfg!(feature = "showcase") { 1 } else { 60 };
/// The longest the thread waits without refreshing the status, so the page
/// sees the position move even during a long gap in the feed.
const STATUS_EVERY: Duration = Duration::from_secs(1);

/// The files build.rs found; missing optional ones are empty.
const FILES: [(Stream, &str); 9] = [
    (
        Stream::SessionInfo,
        include_str!(env!("REPLAY_SESSION_INFO")),
    ),
    (
        Stream::TrackStatus,
        include_str!(env!("REPLAY_TRACK_STATUS")),
    ),
    (
        Stream::SessionStatus,
        include_str!(env!("REPLAY_SESSION_STATUS")),
    ),
    (
        Stream::RaceControl,
        include_str!(env!("REPLAY_RACE_CONTROL")),
    ),
    (Stream::DriverList, include_str!(env!("REPLAY_DRIVER_LIST"))),
    (Stream::TopThree, include_str!(env!("REPLAY_TOP_THREE"))),
    (Stream::PitLane, include_str!(env!("REPLAY_PIT_LANE"))),
    (
        Stream::TimingStats,
        include_str!(env!("REPLAY_TIMING_STATS")),
    ),
    (Stream::Overtakes, include_str!(env!("REPLAY_OVERTAKES"))),
];

/// How the web server reaches the replay thread.
pub struct ReplayHandle {
    commands: Sender<ReplayCommand>,
    /// `None` until the thread has read the session.
    status: Mutex<Option<ReplayStatus>>,
}

impl ReplayHandle {
    /// Fails only if the replay thread is gone, which restarts the chip anyway.
    pub fn send(&self, command: ReplayCommand) -> anyhow::Result<()> {
        self.commands.send(command)?;
        Ok(())
    }

    /// `None` while the session is still being read.
    pub fn status(&self) -> Option<ReplayStatus> {
        *self.status.lock().unwrap()
    }

    /// The session folder, e.g. for the page title.
    pub fn name(&self) -> &'static str {
        env!("REPLAY_DIR")
    }
}

static HANDLE: OnceLock<ReplayHandle> = OnceLock::new();

/// The running replay, once `spawn` has been called.
pub fn handle() -> Option<&'static ReplayHandle> {
    HANDLE.get()
}

/// Spawns the thread that plays the session. It starts paused just before
/// lights out; the page's Play button starts it. Reading the session takes a
/// moment, so it happens in the thread, not here: the render loop must not
/// wait for it.
pub fn spawn(tx: SyncSender<Input>, heartbeat: Arc<Heartbeat>) -> anyhow::Result<JoinHandle<()>> {
    let (commands, rx) = mpsc::channel();
    let handle = ReplayHandle {
        commands,
        status: Mutex::new(None),
    };
    if HANDLE.set(handle).is_err() {
        anyhow::bail!("replay started twice");
    }

    let thread = thread::Builder::new()
        .name("replay".into())
        .stack_size(crate::config::stack::REPLAY)
        .spawn(move || {
            run(&rx, &tx, &heartbeat);
            log::warn!("render loop is gone, stopping the replay");
        })?;
    Ok(thread)
}

/// Returns only when the render loop is gone.
fn run(rx: &Receiver<ReplayCommand>, tx: &SyncSender<Input>, heartbeat: &Heartbeat) {
    let handle = HANDLE.get().expect("set in spawn");
    let mut player = Player::read();
    loop {
        heartbeat.beat(Instant::now());
        let Some(wait) = player.step(tx) else {
            return;
        };
        player.publish(handle);
        if player.wait_for_command(rx, wait) {
            player.publish(handle);
        }
    }
}

/// The replay, and whether its end has been logged yet.
struct Player {
    replayer: Replayer<'static>,
    finished_logged: bool,
}

impl Player {
    /// Reads the embedded session, paused just before lights out.
    fn read() -> Self {
        let started = Instant::now();
        let replayer = Replayer::new(&FILES, SPEED, started);
        let status = replayer.status(Instant::now());
        log::info!(
            "replay: {} ({} of feed), paused at {}, read in {} ms",
            env!("REPLAY_DIR"),
            clock(status.end_ms),
            clock(status.position_ms),
            started.elapsed().as_millis(),
        );
        Player {
            replayer,
            finished_logged: false,
        }
    }

    /// Sends what's due. Returns how long to wait before the next step, or
    /// `None` when the render loop is gone.
    fn step(&mut self, tx: &SyncSender<Input>) -> Option<Duration> {
        Some(match self.replayer.step(Instant::now()) {
            Step::Send(inputs) => {
                for input in inputs {
                    match input {
                        Input::Race { event, .. } => log::info!("replay: {event:?}"),
                        other => log::info!("replay: {other:?}"),
                    }
                    tx.send(input).ok()?;
                }
                Duration::ZERO
            }
            Step::Wait(wait) => wait.min(STATUS_EVERY),
            Step::Paused => STATUS_EVERY,
            Step::Finished => {
                if !self.finished_logged {
                    log::info!("replay: finished");
                    self.finished_logged = true;
                }
                STATUS_EVERY
            }
            Step::Skipped(e) => {
                log::warn!("replay: skipped {e}");
                Duration::ZERO
            }
        })
    }

    /// Waits up to `wait` for a command from the page and carries it out.
    /// Waiting here, not in `thread::sleep`, so a command acts at once.
    /// Whether one came.
    fn wait_for_command(&mut self, rx: &Receiver<ReplayCommand>, wait: Duration) -> bool {
        match rx.recv_timeout(wait) {
            Ok(command) => {
                let took = Instant::now();
                self.replayer.command(command, took);
                log::info!("replay: {command:?} ({} ms)", took.elapsed().as_millis());
                self.finished_logged = false;
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            // The sender lives in a static, so this can't happen.
            Err(RecvTimeoutError::Disconnected) => {
                thread::sleep(wait);
                false
            }
        }
    }

    /// Shares the position with the page.
    fn publish(&self, handle: &ReplayHandle) {
        *handle.status.lock().unwrap() = Some(self.replayer.status(Instant::now()));
    }
}

/// Milliseconds as `H:MM:SS`, like the offsets in the files.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}
