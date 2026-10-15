//! The firmware's start and its render loop. At boot: the update trial, the
//! LEDs, the settings, then the threads (`spawn_threads`). From then on the main
//! task is the render loop: inputs in, a frame out every 20 ms, the snapshot
//! for the web page, and once a second `supervise`.

use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use esp_idf_svc::hal::{modem::Modem, peripherals::Peripherals, task::watchdog::TWDT};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use f1_core::{
    controller::Controller, frame::Frame, frame_stats::FrameStats, heartbeat::Heartbeat,
    input::Input, snapshot::Snapshot,
};
use io::{led::LedOutput, storage::SettingsStore, system::restart};

mod config;
mod io;
mod net;
mod web;

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
/// How often the render loop checks that all threads are still running.
const THREAD_CHECK_EVERY: Duration = Duration::from_secs(1);
/// A thread that hasn't beaten for this long has hung. The threads beat
/// between their network steps, so this covers the longest single one,
/// checked below: waiting for the TLS lock while the other thread holds it,
/// then one HTTPS request; or the live thread's pause between reconnects.
const THREAD_SILENCE_LIMIT: Duration = Duration::from_secs(180);
const _: () = {
    let request = io::http::LONGEST_REQUEST.as_secs();
    let hold = io::live::LONGEST_TLS_HOLD.as_secs();
    let lock_wait = if hold > request { hold } else { request };
    let longest = lock_wait + request;
    let longest = if io::live::MAX_RETRY.as_secs() > longest {
        io::live::MAX_RETRY.as_secs()
    } else {
        longest
    };
    assert!(
        THREAD_SILENCE_LIMIT.as_secs() > longest,
        "THREAD_SILENCE_LIMIT is shorter than a network step"
    );
};

fn main() {
    esp_idf_svc::sys::link_patches();
    io::clock::set_timezone();
    // The serial log, plus the last lines for the web page.
    io::weblog::init();
    if let Err(e) = run() {
        log::error!("startup failed, restarting: {e:#}");
        restart();
    }
}

fn run() -> anyhow::Result<()> {
    let booted = Instant::now();
    let (lamp, rx, threads, twdt) = start()?;
    render_loop(lamp, &rx, &threads, twdt, booted)
}

/// Everything before the first frame: the trial check, the settings, the
/// LEDs and the threads. Also hands over the task watchdog, for the render
/// loop.
fn start() -> anyhow::Result<(Lamp, Receiver<Input>, [Watched; 2], TWDT<'static>)> {
    if config::wifi::SSID.is_empty() {
        log::warn!("WiFi SSID is empty: set wifi_ssid in f1/cfg.toml");
    }
    log::info!("SSID: {}", config::wifi::SSID);

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    // Before anything that could crash: a new firmware on trial is counted
    // here, and switched back after three boots that never got healthy
    // (a crash, or no health within `HEALTHY_WITHIN`, see `supervise`).
    io::ota::check_boot(nvs.clone());
    #[cfg(feature = "crash-test")]
    crash_test();

    let storage = SettingsStore::new(nvs.clone())?;
    // The legacy RMT channel: the only API ws2812-esp32-rmt-driver 0.14 takes.
    #[allow(deprecated)]
    let mut leds = LedOutput::new(peripherals.rmt.channel0, peripherals.pins.gpio2)?;
    leds.write(&Frame::new())?;

    let mut controller = Controller::new(Instant::now());
    controller.apply(Input::Settings(storage.load()), Instant::now());
    // Written by the render loop, read by the web server.
    let snapshot = Arc::new(Mutex::new(controller.snapshot()));

    // Each thread and the web server send through their own clone.
    let (tx, rx) = mpsc::sync_channel::<Input>(config::INPUT_QUEUE);
    let threads = spawn_threads(peripherals.modem, sys_loop, nvs, &tx, &snapshot)?;

    let lamp = Lamp {
        last_snapshot: controller.snapshot(),
        controller,
        leds,
        led_failures: 0,
        storage,
        frame: Frame::new(),
        snapshot,
        stats: FrameStats::new(Instant::now()),
    };
    Ok((lamp, rx, threads, peripherals.twdt))
}

/// The main task from here on: a frame every `FRAME_INTERVAL`, and a look
/// at the threads every `THREAD_CHECK_EVERY`.
fn render_loop(
    mut lamp: Lamp,
    rx: &Receiver<Input>,
    threads: &[Watched],
    twdt: TWDT<'static>,
    booted: Instant,
) -> ! {
    // It must not wait for the network threads' work. Why 6:
    // `config::RENDER_PRIORITY`.
    io::system::set_own_priority(config::RENDER_PRIORITY);
    // Both live as long as the loop, which never ends; without them the
    // lamp still runs, unwatched.
    let mut watchdog = io::system::watchdog(twdt, config::RENDER_WATCHDOG)
        .inspect_err(|e| log::warn!("watchdog not started: {e:#}"))
        .ok();
    let mut watch = watchdog.as_mut().and_then(|w| {
        w.watch_current_task()
            .inspect_err(|e| log::warn!("watchdog not watching: {e}"))
            .ok()
    });
    let mut next_check = Instant::now() + THREAD_CHECK_EVERY;
    let mut pacer = io::system::Pacer::new(FRAME_INTERVAL);
    loop {
        if let Some(watch) = &mut watch {
            // Can't fail once the task is watched.
            let _ = watch.feed();
        }
        let now = Instant::now();
        if now >= next_check {
            supervise(threads, booted, now);
            next_check = now + THREAD_CHECK_EVERY;
        }
        lamp.take_inputs(rx, now);
        lamp.show(now);
        io::racelog::record(lamp.controller.take_log());
        lamp.publish_snapshot();
        lamp.storage.save_if_due(now);
        pacer.wait();
    }
}

/// Starts the net thread and the live-feed thread, with what
/// they share, each watched by `supervise` through its heartbeat.
fn spawn_threads(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: &SyncSender<Input>,
    snapshot: &Arc<Mutex<Snapshot>>,
) -> anyhow::Result<[Watched; 2]> {
    // Set by the net thread, read by the live-feed thread.
    let live = Arc::new(io::live::LiveControl::default());
    // Set from the web page, read by the live-feed thread.
    let tokens = Arc::new(io::token_store::TokenKeeper::new(nvs.clone())?);
    let net_beat = Arc::new(Heartbeat::new(Instant::now()));
    let feed_beat = Arc::new(Heartbeat::new(Instant::now()));
    Ok([
        Watched {
            name: "net",
            handle: net::spawn(
                modem,
                sys_loop,
                nvs,
                tx.clone(),
                net::Shared {
                    snapshot: Arc::clone(snapshot),
                    live: Arc::clone(&live),
                    tokens: Arc::clone(&tokens),
                },
                Arc::clone(&net_beat),
            )?,
            heartbeat: net_beat,
        },
        Watched {
            name: "live",
            handle: io::live::spawn(tx.clone(), live, tokens, Arc::clone(&feed_beat))?,
            heartbeat: feed_beat,
        },
    ])
}

/// The once-a-second checks: every thread still running, and a new firmware
/// on trial healthy in time. Restarts the chip when one fails. Here in the
/// render loop, not in the net thread: that thread may be the one stuck.
fn supervise(threads: &[Watched], booted: Instant, now: Instant) {
    check_threads(threads, now);
    if io::ota::trial_overdue(now.duration_since(booted)) {
        log::error!(
            "ota: new firmware not healthy {} s after boot, restarting",
            now.duration_since(booted).as_secs()
        );
        restart();
    }
}

/// What the render loop owns: the decisions, the LEDs, and the settings to
/// save.
struct Lamp {
    controller: Controller,
    leds: LedOutput<'static>,
    /// Frames in a row that didn't reach the LEDs.
    led_failures: u32,
    storage: SettingsStore,
    frame: Frame,
    /// Shared with the web server.
    snapshot: Arc<Mutex<Snapshot>>,
    /// The last one shared, so only changes are copied and logged.
    last_snapshot: Snapshot,
    /// How smoothly frames come, logged once a minute.
    stats: FrameStats,
}

impl Lamp {
    /// Applies every input waiting; new settings are also saved, later.
    fn take_inputs(&mut self, rx: &Receiver<Input>, now: Instant) {
        while let Ok(input) = rx.try_recv() {
            log::debug!("input: {input:?}");
            self.controller.apply(input, now);
            if let Input::Settings(s) = input {
                self.storage.changed(s);
            }
        }
    }

    /// Renders a frame and sends it to the LEDs.
    fn show(&mut self, now: Instant) {
        self.controller.render(now, &mut self.frame);
        match self.leds.write(&self.frame) {
            Ok(()) => self.led_failures = 0,
            Err(e) => {
                self.led_failures += 1;
                if self.led_failures == 1 {
                    log::warn!("{e}");
                }
                if self.led_failures >= config::led::FAILURES_BEFORE_RESTART {
                    log::error!(
                        "LEDs failing for {} frames ({e}), restarting",
                        self.led_failures
                    );
                    restart();
                }
            }
        }
        if let Some(r) = self.stats.frame(now) {
            io::system::MAIN_STACK.measure();
            log::info!(
                "frames: {} in {} s, longest gap {} ms",
                r.frames,
                r.over.as_secs(),
                r.longest_gap.as_millis()
            );
        }
    }

    /// Shares the snapshot with the web server when it changed. After
    /// `show`, which ticks, so expired overlays don't show.
    fn publish_snapshot(&mut self) {
        let snap = self.controller.snapshot();
        if snap == self.last_snapshot {
            return;
        }
        *self.snapshot.lock().unwrap() = snap;
        let shown = |s: &Snapshot| (s.layer, s.phase, s.net, s.override_active);
        if shown(&snap) != shown(&self.last_snapshot) {
            log::info!(
                "lamp: {} ({:?}, {:?}{})",
                snap.layer,
                snap.phase,
                snap.net,
                if snap.override_active {
                    ", override"
                } else {
                    ""
                }
            );
        }
        self.last_snapshot = snap;
    }
}

/// A spawned thread and the heartbeat it keeps.
struct Watched {
    name: &'static str,
    handle: JoinHandle<()>,
    heartbeat: Arc<Heartbeat>,
}

/// Restarts the chip if any thread has ended, for whatever reason (a panic,
/// an error, or a normal return), or has hung: no heartbeat for
/// `THREAD_SILENCE_LIMIT`. The lamp can't work with a thread missing, and a
/// fresh boot is the simplest way to get it back.
fn check_threads(threads: &[Watched], now: Instant) {
    for thread in threads {
        if thread.handle.is_finished() {
            log::error!("thread {} stopped, restarting", thread.name);
            restart();
        }
        let silent = thread.heartbeat.silent_for(now);
        if silent >= THREAD_SILENCE_LIMIT {
            log::error!(
                "thread {} silent for {} s, restarting",
                thread.name,
                silent.as_secs()
            );
            restart();
        }
    }
}

/// With `--features crash-test`: a firmware that crashes 10 s after every
/// boot, to see the update fallback switch back to the firmware before it.
#[cfg(feature = "crash-test")]
fn crash_test() {
    log::warn!("crash-test: this firmware crashes in 10 s, on every boot");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        panic!("crash-test");
    });
}
