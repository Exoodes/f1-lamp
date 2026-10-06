//! The network thread: WiFi (and reconnecting), SNTP, mDNS, starting the web
//! server, the OpenF1 calendar and winner, the session phase, and whether the
//! live feed is wanted. One loop of jobs, each with its own deadline
//! (`Net::round`).

use std::{
    sync::{mpsc::SyncSender, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Context;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop, hal::modem::Modem, http::server::EspHttpServer, mdns::EspMdns,
    nvs::EspDefaultNvsPartition, sntp::EspSntp,
};
use f1_core::{
    backoff::Backoff,
    color::Rgb,
    heartbeat::Heartbeat,
    input::{Input, NetStatus, RaceEvent, SessionPhase},
    openf1::{self, DriverDto, ResultDto, SessionDto, CALENDAR_DAYS},
    schedule::{self, Session, WinnerLookup, CACHED_SESSIONS},
    snapshot::Snapshot,
};

use crate::{
    config,
    io::{
        clock, http, live::LiveControl, mdns, ota, storage::ScheduleStore, system,
        token_store::TokenKeeper, wifi,
    },
    web, WIFI_PSK, WIFI_SSID,
};

/// What the net thread shares with the other threads.
pub struct Shared {
    /// Written by the render loop, read by the web server.
    pub snapshot: Arc<Mutex<Snapshot>>,
    /// Set here, read by the live-feed thread.
    pub live: Arc<LiveControl>,
    /// Set from the web page, read by the live-feed thread.
    pub tokens: Arc<TokenKeeper>,
}

/// Spawns the network thread, which owns WiFi and the web server and reports
/// the network status.
pub fn spawn(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: SyncSender<Input>,
    shared: Shared,
    heartbeat: Arc<Heartbeat>,
) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("net".into())
        .stack_size(config::stack::NET)
        .spawn(move || {
            if let Err(e) = run(modem, sys_loop, nvs, &tx, shared, &heartbeat) {
                log::error!("network thread stopped: {e:#}");
            }
        })?;
    Ok(handle)
}

/// A new firmware passes its trial once its web server has run this long.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);
/// How often to check that WiFi is still up.
const WIFI_CHECK_EVERY: Duration = Duration::from_secs(5);
/// How often to log the free heap.
const HEAP_LOG_EVERY: Duration = Duration::from_secs(60);
/// Wait before the first reconnect attempt; doubles after each failure.
const FIRST_RETRY_WAIT: Duration = Duration::from_secs(1);
/// The reconnect wait never grows beyond this.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
/// How often to send the time once it's known.
const CLOCK_EVERY: Duration = Duration::from_secs(30);
/// How often to look for the first valid time after boot.
const CLOCK_WAITING_EVERY: Duration = Duration::from_secs(1);
/// How often to download the calendar; the window moves with the date.
const CALENDAR_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Wait after the first failed download; doubles after each further failure.
/// OpenF1 refuses free requests for a whole live session (HTTP 401), so
/// retrying every minute would mean ~120 wasted TLS handshakes per race.
const FIRST_CALENDAR_RETRY: Duration = Duration::from_secs(60);
/// The calendar retry wait never grows beyond this.
const MAX_CALENDAR_RETRY: Duration = Duration::from_secs(30 * 60);
/// How often to check whether WiFi and the clock are ready for a download.
const CALENDAR_WAITING_EVERY: Duration = Duration::from_secs(5);
/// How often to check for a valid clock before the first phase.
const PHASE_WAITING_EVERY: Duration = Duration::from_secs(1);
/// How often to look for a race's winner while it's due; results can take a
/// while to appear after the flag.
const WINNER_EVERY: Duration = Duration::from_secs(60);

/// When each of the thread's jobs is next due.
struct Deadlines {
    wifi_check: Instant,
    heap_log: Instant,
    clock: Instant,
    calendar: Instant,
    phase: Instant,
    winner: Instant,
}

impl Deadlines {
    fn earliest(&self) -> Instant {
        self.wifi_check
            .min(self.heap_log)
            .min(self.clock)
            .min(self.calendar)
            .min(self.phase)
            .min(self.winner)
    }
}

impl Deadlines {
    /// Every job due at once.
    fn all(now: Instant) -> Self {
        Deadlines {
            wifi_check: now,
            heap_log: now,
            clock: now,
            calendar: now,
            phase: now,
            winner: now,
        }
    }
}

/// What the scheduler knows: the sessions, the copy of them in flash, and
/// the phase it last sent.
struct Schedule {
    sessions: Vec<Session>,
    store: ScheduleStore,
    /// What `store` holds, so an unchanged list isn't written again.
    saved: Vec<Session>,
    sent_phase: Option<SessionPhase>,
}

impl Schedule {
    /// The sessions saved in flash: a reboot during a session (when OpenF1
    /// refuses free requests) still knows what's on.
    fn load(store: ScheduleStore) -> Self {
        let sessions = store.load();
        Schedule {
            saved: sessions.clone(),
            sessions,
            store,
            sent_phase: None,
        }
    }

    /// Swaps in a freshly downloaded list, and saves the part of it that
    /// still matters if that changed.
    fn replace(&mut self, sessions: Vec<Session>, now_unix: i64) {
        let relevant = schedule::still_relevant(&sessions, now_unix, CACHED_SESSIONS);
        if relevant != self.saved {
            match self.store.save(&relevant) {
                Ok(()) => {
                    log::info!("schedule saved: {} sessions", relevant.len());
                    self.saved = relevant;
                }
                // Not fatal: the next download tries again.
                Err(e) => log::warn!("{e:#}"),
            }
        }
        self.sessions = sessions;
    }

    /// Works out the phase at `now_unix` and sends it, but only when it
    /// changed. Returns it either way.
    fn update_phase(
        &mut self,
        now_unix: i64,
        tx: &SyncSender<Input>,
    ) -> anyhow::Result<SessionPhase> {
        let phase = schedule::phase_at(&self.sessions, now_unix);
        // A replay owns the phase; two producers would fight over it.
        if cfg!(feature = "player") || self.sent_phase == Some(phase) {
            return Ok(phase);
        }
        tx.send(Input::Phase(phase))
            .context("render loop is gone")?;
        self.sent_phase = Some(phase);
        log_phase(&self.sessions, now_unix);
        Ok(phase)
    }
}

/// The thread's body. Returns only on a fatal error.
fn run(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: &SyncSender<Input>,
    shared: Shared,
    heartbeat: &Heartbeat,
) -> anyhow::Result<()> {
    let mut net = Net::start(modem, sys_loop, nvs, tx, shared)?;
    loop {
        let now = Instant::now();
        heartbeat.beat(now);
        net.round(now)?;
        thread::sleep(net.due.earliest().saturating_duration_since(Instant::now()));
    }
}

/// The net thread's state from one round of its loop to the next.
struct Net<'a> {
    tx: &'a SyncSender<Input>,
    shared: Shared,
    wifi: wifi::Wifi,
    /// The status last sent, so each change is sent once.
    status: Option<NetStatus>,
    wifi_retry: Backoff,
    /// Dropping it stops mDNS.
    _mdns: EspMdns,
    /// Kept alive here: time only syncs while this exists. Started once WiFi
    /// is up.
    sntp: Option<EspSntp<'static>>,
    /// Same here: dropping it stops the server. Started once WiFi is up, as
    /// the TCP/IP stack doesn't exist before WiFi is created.
    server: Option<EspHttpServer<'static>>,
    /// When the web server started; a new firmware is healthy once it has
    /// served for `HEALTHY_AFTER`, and can then take the next update.
    server_since: Option<Instant>,
    healthy: bool,
    schedule: Schedule,
    calendar_retry: Backoff,
    /// The session whose winner was sent, so each race is shown once.
    winner_sent: Option<u32>,
    due: Deadlines,
}

impl<'a> Net<'a> {
    /// Loads the cached schedule and starts WiFi (not yet connected) and
    /// mDNS.
    fn start(
        modem: Modem<'static>,
        sys_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        tx: &'a SyncSender<Input>,
        shared: Shared,
    ) -> anyhow::Result<Self> {
        let mut status = None;
        report(tx, &mut status, NetStatus::Connecting)?;
        let schedule = Schedule::load(ScheduleStore::new(nvs.clone())?);
        let wifi = wifi::create(modem, sys_loop, nvs, WIFI_SSID, WIFI_PSK)?;
        let mdns = mdns::start().context("start mDNS")?;
        clock::set_timezone();
        Ok(Net {
            tx,
            shared,
            wifi,
            status,
            wifi_retry: Backoff::new(FIRST_RETRY_WAIT, MAX_RETRY_WAIT),
            _mdns: mdns,
            sntp: None,
            server: None,
            server_since: None,
            healthy: false,
            schedule,
            calendar_retry: Backoff::new(FIRST_CALENDAR_RETRY, MAX_CALENDAR_RETRY),
            winner_sent: None,
            due: Deadlines::all(Instant::now()),
        })
    }

    /// Runs every job that is due at `now`.
    fn round(&mut self, now: Instant) -> anyhow::Result<()> {
        if now >= self.due.wifi_check {
            self.due.wifi_check = self.check_wifi()?;
        }
        self.start_services(now)?;
        self.check_trial(now);
        if now >= self.due.clock {
            self.due.clock = self.send_clock(now)?;
        }
        // Before the phase: a new calendar makes the phase due at once.
        if now >= self.due.calendar {
            self.due.calendar = self.update_calendar(now);
        }
        if now >= self.due.phase {
            self.due.phase = self.update_phase(now)?;
        }
        if now >= self.due.winner {
            self.look_for_winner()?;
            self.due.winner = now + WINNER_EVERY;
        }
        if now >= self.due.heap_log {
            let heap = system::heap();
            log::info!(
                "heap: {} B free, {} B lowest since boot",
                heap.free,
                heap.lowest
            );
            self.due.heap_log = now + HEAP_LOG_EVERY;
        }
        Ok(())
    }

    fn wifi_up(&self) -> bool {
        self.wifi.is_up().unwrap_or(false)
    }

    /// Reconnects if WiFi is down. Returns when the next check is due: soon
    /// when all is well, after a wait that doubles with each failure when it
    /// isn't.
    fn check_wifi(&mut self) -> anyhow::Result<Instant> {
        if self.wifi_up() {
            report(self.tx, &mut self.status, NetStatus::Online)?;
            return Ok(Instant::now() + WIFI_CHECK_EVERY);
        }

        report(self.tx, &mut self.status, NetStatus::Connecting)?;
        log::info!("connecting to {WIFI_SSID}");
        match wifi::connect(&mut self.wifi) {
            Ok(()) => {
                report(self.tx, &mut self.status, NetStatus::Online)?;
                self.wifi_retry.reset();
                Ok(Instant::now() + WIFI_CHECK_EVERY)
            }
            Err(e) => {
                let wait = self.wifi_retry.next_wait();
                log::warn!("wifi: {e:#}; retrying in {} s", wait.as_secs());
                Ok(Instant::now() + wait)
            }
        }
    }

    /// SNTP and the web server, the first time WiFi is up.
    fn start_services(&mut self, now: Instant) -> anyhow::Result<()> {
        if self.sntp.is_none() && self.wifi_up() {
            self.sntp = Some(EspSntp::new_default().context("start SNTP")?);
            log::info!("SNTP started");
        }
        if self.server.is_none() && self.wifi_up() {
            let Shared {
                snapshot, tokens, ..
            } = &self.shared;
            self.server = Some(
                web::start(Arc::clone(snapshot), self.tx.clone(), Arc::clone(tokens))
                    .context("start web server")?,
            );
            self.server_since = Some(now);
        }
        Ok(())
    }

    /// Ends a new firmware's trial once its web server has run long enough.
    fn check_trial(&mut self, now: Instant) {
        let served_long_enough = self
            .server_since
            .is_some_and(|t| now.duration_since(t) >= HEALTHY_AFTER);
        if !self.healthy && served_long_enough {
            ota::mark_healthy();
            self.healthy = true;
        }
    }

    /// Sends the local time once the clock is set. Returns when it's next due.
    fn send_clock(&self, now: Instant) -> anyhow::Result<Instant> {
        Ok(match clock::minute_of_day() {
            // Until the first sync nothing is sent, so the core keeps "not night".
            None => now + CLOCK_WAITING_EVERY,
            Some(minute_of_day) => {
                log::info!("clock: {:02}:{:02}", minute_of_day / 60, minute_of_day % 60);
                self.tx
                    .send(Input::Clock { minute_of_day })
                    .context("render loop is gone")?;
                now + CLOCK_EVERY
            }
        })
    }

    /// Downloads the calendar once WiFi and the clock are ready. Returns when
    /// it's next due. Never fails: the lamp works without a calendar.
    fn update_calendar(&mut self, now: Instant) -> Instant {
        let (Some(unix), true) = (clock::unix_now(), self.wifi_up()) else {
            return now + CALENDAR_WAITING_EVERY;
        };
        match fetch_calendar(unix) {
            Ok(sessions) => {
                self.schedule.replace(sessions, unix);
                log_calendar(&self.schedule.sessions, unix);
                // Work out the phase straight away with the new list.
                self.due.phase = now;
                self.calendar_retry.reset();
                now + CALENDAR_EVERY
            }
            Err(e) => {
                let wait = self.calendar_retry.next_wait();
                log::warn!("calendar: {e:#}; retrying in {} s", wait.as_secs());
                now + wait
            }
        }
    }

    /// Sends the phase if it changed and says whether the live feed is
    /// wanted. Returns when it's next due: at the next boundary, or sooner.
    fn update_phase(&mut self, now: Instant) -> anyhow::Result<Instant> {
        // No valid time yet: the schedule can't be read without it.
        let Some(unix) = clock::unix_now() else {
            return Ok(now + PHASE_WAITING_EVERY);
        };
        let phase = self.schedule.update_phase(unix, self.tx)?;
        self.shared
            .live
            .set_wanted(cfg!(feature = "live-now") || schedule::feed_wanted(phase));
        Ok(now + schedule::phase_wait(&self.schedule.sessions, unix))
    }

    /// The OpenF1 winner lookup, once WiFi and the clock are ready.
    fn look_for_winner(&mut self) -> anyhow::Result<()> {
        if let (Some(unix), true) = (clock::unix_now(), self.wifi_up()) {
            check_winner(
                &self.schedule.sessions,
                unix,
                &mut self.winner_sent,
                &self.shared.live,
                self.tx,
            )?;
        }
        Ok(())
    }
}

/// If a race is in its winner window and its winner hasn't been sent yet,
/// looks it up and sends `RaceEvent::Winner`. Failures only log: the caller
/// tries again a minute later.
fn check_winner(
    sessions: &[Session],
    now_unix: i64,
    sent: &mut Option<u32>,
    live: &LiveControl,
    tx: &SyncSender<Input>,
) -> anyhow::Result<()> {
    let key = match schedule::winner_lookup(sessions, now_unix, *sent, live.winner_shown()) {
        WinnerLookup::Nothing => return Ok(()),
        WinnerLookup::ShownByFeed(key) => {
            log::info!("winner: session {key} already shown from the live feed");
            *sent = Some(key);
            return Ok(());
        }
        WinnerLookup::Fetch(key) => key,
    };

    match fetch_winner(key) {
        Ok(Some((driver, team_color))) => {
            log::info!("winner: #{driver} ({team_color}), session {key}");
            tx.send(Input::Race {
                event: RaceEvent::Winner { driver, team_color },
                received: Instant::now(),
            })
            .context("render loop is gone")?;
            *sent = Some(key);
        }
        Ok(None) => log::info!(
            "winner: session {key} not published yet; retrying in {} s",
            WINNER_EVERY.as_secs()
        ),
        Err(e) => log::warn!("winner: {e:#}; retrying in {} s", WINNER_EVERY.as_secs()),
    }
    Ok(())
}

/// The winner of `session_key` and their team colour; `None` until OpenF1 has
/// published the result (it answers 404 until then).
fn fetch_winner(session_key: u32) -> anyhow::Result<Option<(u8, Rgb)>> {
    let results =
        http::get_json::<Vec<ResultDto>>(&openf1::results_url(session_key))?.unwrap_or_default();
    let Some(driver) = openf1::winner(&results) else {
        return Ok(None);
    };
    let drivers =
        http::get_json::<Vec<DriverDto>>(&openf1::drivers_url(session_key))?.unwrap_or_default();
    // Without a colour, try again later rather than guess one.
    Ok(openf1::team_colours(drivers)
        .get(&driver)
        .map(|&colour| (driver, colour)))
}

/// Downloads the sessions of the next `CALENDAR_DAYS` days, ready to use.
fn fetch_calendar(now_unix: i64) -> anyhow::Result<Vec<Session>> {
    let url = openf1::sessions_url(now_unix);
    log::info!("calendar: GET {url}");
    // `None` is OpenF1's 404 for "no sessions", e.g. in the winter break.
    let dtos = http::get_json::<Vec<SessionDto>>(&url)?.unwrap_or_default();
    Ok(openf1::usable_sessions(dtos))
}

fn log_calendar(sessions: &[Session], now_unix: i64) {
    log::info!(
        "calendar: {} sessions in the next {CALENDAR_DAYS} days",
        sessions.len()
    );
    log_next(sessions, now_unix);
}

/// `phase: Live (Race, session 11731)`, then the next session.
fn log_phase(sessions: &[Session], now_unix: i64) {
    match schedule::current_session(sessions, now_unix) {
        Some((phase, s)) => log::info!("phase: {phase:?} ({:?}, session {})", s.kind, s.key),
        None => log::info!("phase: Idle"),
    }
    log_next(sessions, now_unix);
}

/// The next session and a countdown to it.
fn log_next(sessions: &[Session], now_unix: i64) {
    match schedule::next_session(sessions, now_unix) {
        Some(next) => log::info!(
            "next: {:?} (session {}) in {}",
            next.kind,
            next.key,
            in_words(next.start - now_unix)
        ),
        None => log::info!("next: no session coming up"),
    }
}

/// `2 d 5 h 48 min`, or `3 h 20 min` when under a day.
fn in_words(seconds: i64) -> String {
    let minutes = seconds / 60;
    let (days, hours, minutes) = (minutes / (24 * 60), minutes / 60 % 24, minutes % 60);
    if days > 0 {
        format!("{days} d {hours} h {minutes} min")
    } else {
        format!("{hours} h {minutes} min")
    }
}

/// Sends `new` unless it's the status we last sent.
fn report(
    tx: &SyncSender<Input>,
    last: &mut Option<NetStatus>,
    new: NetStatus,
) -> anyhow::Result<()> {
    if *last == Some(new) {
        return Ok(());
    }
    tx.send(Input::Net(new)).context("render loop is gone")?;
    *last = Some(new);
    Ok(())
}
