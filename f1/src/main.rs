use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use esp_idf_svc::hal::{modem::Modem, peripherals::Peripherals};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use f1_core::{
    controller::Controller, frame::Frame, frame_stats::FrameStats, heartbeat::Heartbeat,
    input::Input, snapshot::Snapshot,
};
use io::{led::LedOutput, storage::SettingsStore};

mod config;
mod io;
mod net;
#[cfg(feature = "player")]
mod replay;
mod web;

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
/// How often the render loop checks that all threads are still running.
const THREAD_CHECK_EVERY: Duration = Duration::from_secs(1);
/// A thread that hasn't beaten for this long has hung. Well above the
/// longest normal wait: two 15 s HTTPS requests plus the TLS lock, or the
/// live thread's 60 s pause between reconnects.
const THREAD_SILENCE_LIMIT: Duration = Duration::from_secs(180);

// WiFi credentials from f1/cfg.toml, passed in by build.rs.
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PSK: &str = env!("WIFI_PSK");

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    let booted = Instant::now();

    // The serial log, plus the last lines for the web page.
    io::weblog::init();
    if WIFI_SSID.is_empty() {
        log::warn!("WiFi SSID is empty: set wifi_ssid in f1/cfg.toml");
    }
    log::info!("SSID: {WIFI_SSID}");

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    // Before anything that could crash: a new firmware on trial is counted
    // here, and switched back after three boots that never got healthy
    // (a crash, or no health within `HEALTHY_WITHIN`, see `supervise`).
    io::ota::check_boot(nvs.clone())?;
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

    let (tx, rx) = mpsc::sync_channel::<Input>(config::INPUT_QUEUE);
    let threads = spawn_threads(peripherals.modem, sys_loop, nvs, &tx, &snapshot)?;

    let mut lamp = Lamp {
        last_snapshot: controller.snapshot(),
        controller,
        leds,
        storage,
        frame: Frame::new(),
        snapshot,
        stats: FrameStats::new(Instant::now()),
    };
    // From here on the main task is the render loop: it must not wait for
    // the network threads' work. Why 6: `config::RENDER_PRIORITY`.
    io::system::set_own_priority(config::RENDER_PRIORITY);
    let mut next_check = Instant::now() + THREAD_CHECK_EVERY;
    loop {
        let now = Instant::now();
        if now >= next_check {
            supervise(&threads, booted, now);
            next_check = now + THREAD_CHECK_EVERY;
        }
        lamp.take_inputs(&rx, now);
        lamp.show(now);
        lamp.publish_snapshot();
        lamp.storage.save_if_due(now);
        thread::sleep(FRAME_INTERVAL);
    }
}

/// Starts the net thread and the live-feed (or replay) thread, with what
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
        // A replay plays its own events; the live feed would mix in real ones.
        #[cfg(not(feature = "player"))]
        Watched {
            name: "live",
            handle: io::live::spawn(tx.clone(), live, tokens, Arc::clone(&feed_beat))?,
            heartbeat: feed_beat,
        },
        #[cfg(feature = "player")]
        Watched {
            name: "replay",
            handle: replay::spawn(tx.clone(), Arc::clone(&feed_beat))?,
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
            log::info!("input: {input:?}");
            self.controller.apply(input, now);
            if let Input::Settings(s) = input {
                self.storage.changed(s);
            }
        }
    }

    /// Renders a frame and sends it to the LEDs.
    fn show(&mut self, now: Instant) {
        self.controller.render(now, &mut self.frame);
        if let Err(e) = self.leds.write(&self.frame) {
            log::warn!("{e}");
        }
        if let Some(r) = self.stats.frame(now) {
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
        match serde_json::to_string(&snap) {
            Ok(json) => log::info!("snapshot: {json}"),
            Err(e) => log::warn!("snapshot not serialisable: {e}"),
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

pub fn restart() -> ! {
    // SAFETY: `esp_restart` takes no arguments and has no preconditions; it
    // reboots the chip and never returns.
    unsafe { esp_idf_svc::sys::esp_restart() }
}
