use std::{
    sync::{mpsc::SyncSender, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Context;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop, hal::modem::Modem, http::server::EspHttpServer,
    nvs::EspDefaultNvsPartition, sntp::EspSntp,
};
use f1_core::{
    color::Rgb,
    input::{Input, NetStatus, RaceEvent, SessionPhase},
    openf1::{self, DriverDto, ResultDto, SessionDto, CALENDAR_DAYS},
    schedule::{self, Session, CACHED_SESSIONS},
    snapshot::Snapshot,
};

use crate::{
    io::{
        clock, http, live::LiveControl, mdns, storage::ScheduleStore, token_store::TokenKeeper,
        wifi,
    },
    web, WIFI_PSK, WIFI_SSID,
};

/// Spawns the network thread, which owns WiFi and the web server and reports
/// the network status.
pub fn spawn(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: SyncSender<Input>,
    snapshot: Arc<Mutex<Snapshot>>,
    live: Arc<LiveControl>,
    tokens: Arc<TokenKeeper>,
) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("net".into())
        .stack_size(16 * 1024)
        .spawn(move || {
            if let Err(e) = run(modem, sys_loop, nvs, &tx, snapshot, &live, tokens) {
                log::error!("network thread stopped: {e:#}");
            }
        })?;
    Ok(handle)
}

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
/// The phase job wakes at the next boundary (`schedule::next_change`); this
/// is the longest it waits anyway, in case the calendar or the clock changed.
const PHASE_EVERY: Duration = Duration::from_secs(30);
/// Wake this long after a boundary: the clock counts whole seconds, so right
/// on it the old phase could still be read.
const BOUNDARY_MARGIN: Duration = Duration::from_millis(500);
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

/// What the scheduler knows: the sessions and the phase it last sent.
struct Schedule {
    sessions: Vec<Session>,
    sent_phase: Option<SessionPhase>,
}

impl Schedule {
    /// Swaps in a freshly downloaded list.
    fn replace(&mut self, sessions: Vec<Session>) {
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
    snapshot: Arc<Mutex<Snapshot>>,
    live: &LiveControl,
    tokens: Arc<TokenKeeper>,
) -> anyhow::Result<()> {
    let mut status = None;
    report(tx, &mut status, NetStatus::Connecting)?;
    let mut schedule_store = ScheduleStore::new(nvs.clone())?;
    let mut schedule = Schedule {
        sessions: schedule_store.load(),
        sent_phase: None,
    };
    let mut saved = schedule.sessions.clone();
    let mut wifi = wifi::create(modem, sys_loop, nvs, WIFI_SSID, WIFI_PSK)?;
    let _mdns = mdns::start().context("start mDNS")?;
    clock::set_timezone();
    // Kept alive here: time only syncs while this exists. Started once WiFi is up.
    let mut sntp: Option<EspSntp<'static>> = None;
    // Same here: dropping it stops the server. Started once WiFi is up, as the
    // TCP/IP stack doesn't exist before WiFi is created.
    let mut server: Option<EspHttpServer<'static>> = None;

    let mut retry_wait = FIRST_RETRY_WAIT;
    let mut calendar_retry = FIRST_CALENDAR_RETRY;
    // The session whose winner was sent, so each race is shown once.
    let mut winner_sent: Option<u32> = None;
    let now = Instant::now();
    let mut due = Deadlines {
        wifi_check: now,
        heap_log: now,
        clock: now,
        calendar: now,
        phase: now,
        winner: now,
    };

    loop {
        let now = Instant::now();

        if now >= due.wifi_check {
            due.wifi_check = check_wifi(&mut wifi, tx, &mut status, &mut retry_wait)?;
        }
        if sntp.is_none() && wifi.is_up().unwrap_or(false) {
            sntp = Some(EspSntp::new_default().context("start SNTP")?);
            log::info!("SNTP started");
        }
        if server.is_none() && wifi.is_up().unwrap_or(false) {
            server = Some(
                web::start(Arc::clone(&snapshot), tx.clone(), Arc::clone(&tokens))
                    .context("start web server")?,
            );
        }
        if now >= due.clock {
            due.clock = match clock::minute_of_day() {
                // Until the first sync nothing is sent, so the core keeps "not night".
                None => now + CLOCK_WAITING_EVERY,
                Some(minute_of_day) => {
                    log::info!("clock: {:02}:{:02}", minute_of_day / 60, minute_of_day % 60);
                    tx.send(Input::Clock { minute_of_day })
                        .context("render loop is gone")?;
                    now + CLOCK_EVERY
                }
            };
        }
        if now >= due.calendar {
            due.calendar = match (clock::unix_now(), wifi.is_up().unwrap_or(false)) {
                (Some(unix), true) => match fetch_calendar(unix) {
                    Ok(sessions) => {
                        let relevant = schedule::still_relevant(&sessions, unix, CACHED_SESSIONS);
                        if relevant != saved {
                            match schedule_store.save(&relevant) {
                                Ok(()) => {
                                    log::info!("schedule saved: {} sessions", relevant.len());
                                    saved = relevant;
                                }
                                // Not fatal: the next download tries again.
                                Err(e) => log::warn!("{e:#}"),
                            }
                        }
                        log_calendar(&sessions, unix);
                        schedule.replace(sessions);
                        // Work out the phase straight away with the new list.
                        due.phase = now;
                        calendar_retry = FIRST_CALENDAR_RETRY;
                        now + CALENDAR_EVERY
                    }
                    // Not fatal: the lamp works without a calendar, so log and retry.
                    Err(e) => {
                        log::warn!(
                            "calendar: {e:#}; retrying in {} s",
                            calendar_retry.as_secs()
                        );
                        let next = now + calendar_retry;
                        calendar_retry = (calendar_retry * 2).min(MAX_CALENDAR_RETRY);
                        next
                    }
                },
                _ => now + CALENDAR_WAITING_EVERY,
            };
        }
        if now >= due.phase {
            due.phase = match clock::unix_now() {
                Some(unix) => {
                    let phase = schedule.update_phase(unix, tx)?;
                    // The live feed is needed just before and during a session.
                    live.set_wanted(
                        cfg!(feature = "live-now")
                            || matches!(phase, SessionPhase::PreSession | SessionPhase::Live),
                    );
                    let to_boundary = schedule::next_change(&schedule.sessions, unix)
                        .and_then(|at| u64::try_from(at - unix).ok())
                        .map(|secs| Duration::from_secs(secs) + BOUNDARY_MARGIN);
                    now + to_boundary.map_or(PHASE_EVERY, |d| d.min(PHASE_EVERY))
                }
                // No valid time yet: the schedule can't be read without it.
                None => now + PHASE_WAITING_EVERY,
            };
        }
        if now >= due.winner {
            if let (Some(unix), true) = (clock::unix_now(), wifi.is_up().unwrap_or(false)) {
                check_winner(&schedule.sessions, unix, &mut winner_sent, live, tx)?;
            }
            due.winner = now + WINNER_EVERY;
        }
        if now >= due.heap_log {
            let (free, min_free) = heap();
            log::info!("heap: {free} B free, {min_free} B lowest since boot");
            due.heap_log = now + HEAP_LOG_EVERY;
        }

        thread::sleep(due.earliest().saturating_duration_since(Instant::now()));
    }
}

/// Reconnects if WiFi is down. Returns when the next check is due: soon when
/// all is well, after a wait that doubles with each failure when it isn't.
fn check_wifi(
    wifi: &mut wifi::Wifi,
    tx: &SyncSender<Input>,
    status: &mut Option<NetStatus>,
    retry_wait: &mut Duration,
) -> anyhow::Result<Instant> {
    if wifi.is_up().unwrap_or(false) {
        report(tx, status, NetStatus::Online)?;
        return Ok(Instant::now() + WIFI_CHECK_EVERY);
    }

    report(tx, status, NetStatus::Connecting)?;
    log::info!("connecting to {WIFI_SSID}");
    match wifi::connect(wifi) {
        Ok(()) => {
            report(tx, status, NetStatus::Online)?;
            *retry_wait = FIRST_RETRY_WAIT;
            Ok(Instant::now() + WIFI_CHECK_EVERY)
        }
        Err(e) => {
            log::warn!("wifi: {e:#}; retrying in {} s", retry_wait.as_secs());
            let next = Instant::now() + *retry_wait;
            *retry_wait = (*retry_wait * 2).min(MAX_RETRY_WAIT);
            Ok(next)
        }
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
    let Some(race) = schedule::awaiting_winner(sessions, now_unix) else {
        return Ok(());
    };
    if *sent == Some(race.key) {
        return Ok(());
    }
    // The live feed showed it at the flag; OpenF1 is only the fallback.
    if live.winner_shown() == Some(race.key) {
        log::info!(
            "winner: session {} already shown from the live feed",
            race.key
        );
        *sent = Some(race.key);
        return Ok(());
    }

    match fetch_winner(race.key) {
        Ok(Some((driver, team_color))) => {
            log::info!("winner: #{driver} ({team_color}), session {}", race.key);
            tx.send(Input::Race {
                event: RaceEvent::Winner { driver, team_color },
                received: Instant::now(),
            })
            .context("render loop is gone")?;
            *sent = Some(race.key);
        }
        Ok(None) => log::info!(
            "winner: session {} not published yet; retrying in {} s",
            race.key,
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

/// Free heap now, and the lowest it has been since boot, in bytes.
fn heap() -> (u32, u32) {
    // SAFETY: both functions only read ESP-IDF's heap counters; they take no
    // arguments and have no preconditions.
    unsafe {
        (
            esp_idf_svc::sys::esp_get_free_heap_size(),
            esp_idf_svc::sys::esp_get_minimum_free_heap_size(),
        )
    }
}
