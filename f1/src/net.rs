//! The network thread: WiFi (and reconnecting), SNTP, mDNS, starting the web
//! server, the OpenF1 calendar and winner, the session phase, and whether the
//! live feed is wanted. One loop of jobs, each with its own deadline
//! (`Net::round`).

use std::{
    collections::HashSet,
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
    heartbeat::Heartbeat,
    input::{Input, NetStatus, RaceEvent},
    net_schedule::{in_words, instant_at, Calendar, Deadlines, PhaseUpdate},
    openf1::{self, DriverDto, ResultDto, SessionDto, CALENDAR_DAYS},
    schedule::{self, Session, WinnerLookup},
    snapshot::Snapshot,
    weekend::{self, Moved, ResultDue, Results, SessionResult},
};

use crate::{
    config,
    io::{
        clock, http, live::LiveControl, mdns, ota, storage::ScheduleStore, system,
        token_store::TokenKeeper, wifi,
    },
    web,
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
// A trial boot that isn't healthy within `HEALTHY_WITHIN` restarts, so
// getting healthy must fit, with time for WiFi before it.
const _: () = assert!(HEALTHY_AFTER.as_secs() < f1_core::ota_trial::HEALTHY_WITHIN.as_secs());
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
/// How often to look for the weekend's session results for the page while
/// one is missing; OpenF1 publishes them a while after each session.
const RESULTS_EVERY: Duration = Duration::from_secs(5 * 60);

/// The calendar, its copy in flash, and the copy the web page reads.
struct Schedule {
    calendar: Calendar,
    store: ScheduleStore,
    page: Arc<Mutex<Vec<Session>>>,
    /// Sessions that moved since the lamp started, for the web page.
    moved: Arc<Mutex<Moved>>,
}

impl Schedule {
    fn load(store: ScheduleStore) -> Self {
        let saved = store.load();
        Schedule {
            page: Arc::new(Mutex::new(saved.clone())),
            moved: Arc::new(Mutex::new(Moved::new())),
            calendar: Calendar::new(saved),
            store,
        }
    }

    /// Swaps in a freshly downloaded list (cancelled sessions included: the
    /// page shows them, the lamp ignores them), and saves the part of it
    /// that still matters if that changed.
    fn replace(&mut self, sessions: Vec<Session>, now_unix: i64) {
        // Compared with what the page had: cancelled sessions are only there.
        let before = self.page.lock().unwrap().clone();
        for session in weekend::newly_cancelled(&before, &sessions) {
            log::info!("calendar: {} cancelled", title(session));
        }
        let moves = weekend::track_moves(&mut self.moved.lock().unwrap(), &before, &sessions);
        for m in moves {
            if let Some(session) = sessions.iter().find(|s| s.key == m.key) {
                log::info!(
                    "calendar: {} moved from {} to {}",
                    title(session),
                    clock::local_day_time(m.from),
                    clock::local_day_time(m.to)
                );
            }
        }
        let usable = openf1::usable(&sessions);
        *self.page.lock().unwrap() = sessions;
        let Some(relevant) = self.calendar.replace(usable, now_unix) else {
            return;
        };
        match self.store.save(&relevant) {
            Ok(()) => {
                log::info!("schedule saved: {} sessions", relevant.len());
                self.calendar.saved(relevant);
            }
            // Not fatal: the next download tries again.
            Err(e) => log::warn!("{e:#}"),
        }
    }

    /// Works out the phase at `now_unix` and sends it, but only when it
    /// changed. Returns what was found either way.
    fn update_phase(
        &mut self,
        now_unix: i64,
        feed_running: bool,
        tx: &SyncSender<Input>,
    ) -> anyhow::Result<PhaseUpdate> {
        let update = self.calendar.phase(now_unix, feed_running);
        // A replay owns the phase; two producers would fight over it.
        if update.changed && !cfg!(feature = "player") {
            tx.send(Input::Phase(update.phase))
                .context("render loop is gone")?;
            log_phase(self.calendar.followed(), now_unix, feed_running);
        }
        Ok(update)
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
    let mut net = Net::start(modem, sys_loop, nvs, tx, shared, heartbeat)?;
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
    /// Beaten before every network job, so a round of several never looks
    /// like a hang.
    heartbeat: &'a Heartbeat,
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
    /// The clock's first time was logged. The repeats are debug only: every
    /// 30 s, they filled half the web page's log.
    clock_logged: bool,
    schedule: Schedule,
    calendar_retry: Backoff,
    /// The session whose winner was sent, so each race is shown once.
    winner_sent: Option<u32>,
    /// The session and start last sent for the countdown.
    countdown_sent: Option<(u32, i64)>,
    /// The weekend's session results, for the web page.
    results: Arc<Mutex<Results>>,
    /// Sessions OpenF1 had no result for on their last try.
    results_given_up: HashSet<u32>,
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
        heartbeat: &'a Heartbeat,
    ) -> anyhow::Result<Self> {
        let mut status = None;
        report(tx, &mut status, NetStatus::Connecting)?;
        let schedule = Schedule::load(ScheduleStore::new(nvs.clone())?);
        let wifi = wifi::create(modem, sys_loop, nvs, config::wifi::SSID, config::wifi::PSK)?;
        let mdns = mdns::start().context("start mDNS")?;
        Ok(Net {
            tx,
            heartbeat,
            shared,
            wifi,
            status,
            wifi_retry: Backoff::new(FIRST_RETRY_WAIT, MAX_RETRY_WAIT),
            _mdns: mdns,
            sntp: None,
            server: None,
            server_since: None,
            healthy: false,
            clock_logged: false,
            schedule,
            calendar_retry: Backoff::new(FIRST_CALENDAR_RETRY, MAX_CALENDAR_RETRY),
            winner_sent: None,
            countdown_sent: None,
            results: Arc::new(Mutex::new(Results::new())),
            results_given_up: HashSet::new(),
            due: Deadlines::all(Instant::now()),
        })
    }

    /// Runs every job that is due at `now`.
    fn round(&mut self, now: Instant) -> anyhow::Result<()> {
        if now >= self.due.wifi_check {
            self.beat();
            self.due.wifi_check = self.check_wifi()?;
        }
        self.start_services(now)?;
        self.check_trial(now);
        if now >= self.due.clock {
            self.due.clock = self.send_clock(now)?;
        }
        // Before the phase: a new calendar makes the phase due at once.
        if now >= self.due.calendar {
            self.beat();
            self.due.calendar = self.update_calendar(now);
        }
        if now >= self.due.phase {
            self.due.phase = self.update_phase(now)?;
        }
        if now >= self.due.winner {
            self.beat();
            self.look_for_winner()?;
            self.due.winner = now + WINNER_EVERY;
        }
        if now >= self.due.results {
            self.beat();
            self.due.results = self.update_results(now);
        }
        if now >= self.due.heap_log {
            system::NET_STACK.measure();
            let heap = system::heap();
            let stack = |s: &system::StackLeft| s.get().map_or("?".into(), |b| format!("{b} B"));
            let nvs =
                system::nvs_use().map_or("?".into(), |n| format!("{} of {}", n.used, n.total));
            log::info!(
                "heap: {} B free, {} B lowest since boot, largest block {} B;                  stack left: main {}, net {}, live {}; nvs entries used: {}",
                heap.free,
                heap.lowest,
                heap.largest_block,
                stack(&system::MAIN_STACK),
                stack(&system::NET_STACK),
                stack(&system::LIVE_STACK),
                nvs
            );
            self.due.heap_log = now + HEAP_LOG_EVERY;
        }
        Ok(())
    }

    fn beat(&self) {
        self.heartbeat.beat(Instant::now());
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
        log::info!("connecting to {}", config::wifi::SSID);
        match wifi::connect(&mut self.wifi) {
            Ok(()) => {
                report(self.tx, &mut self.status, NetStatus::Online)?;
                self.wifi_retry.reset();
                self.shared.live.wifi_reconnected();
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
                web::start(
                    Arc::clone(snapshot),
                    Arc::clone(&self.schedule.page),
                    Arc::clone(&self.schedule.moved),
                    Arc::clone(&self.results),
                    self.tx.clone(),
                    Arc::clone(tokens),
                )
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
    fn send_clock(&mut self, now: Instant) -> anyhow::Result<Instant> {
        Ok(match clock::minute_of_day() {
            // Until the first sync nothing is sent, so the core keeps "not night".
            None => now + CLOCK_WAITING_EVERY,
            Some(minute_of_day) => {
                let level = if self.clock_logged {
                    log::Level::Debug
                } else {
                    log::Level::Info
                };
                log::log!(
                    level,
                    "clock: {:02}:{:02}",
                    minute_of_day / 60,
                    minute_of_day % 60
                );
                self.clock_logged = true;
                self.tx
                    .send(Input::Clock { minute_of_day })
                    .context("render loop is gone")?;
                now + CLOCK_EVERY
            }
        })
    }

    /// Downloads the calendar once WiFi and the clock are ready, but not
    /// just before or during a session. Returns when it's next due. Never
    /// fails: the lamp works without a calendar.
    fn update_calendar(&mut self, now: Instant) -> Instant {
        let (Some(unix), true) = (clock::unix_now(), self.wifi_up()) else {
            return now + CALENDAR_WAITING_EVERY;
        };
        let feed_running = self.shared.live.feed_running();
        if !self.schedule.calendar.may_download(unix, feed_running) {
            log::info!("calendar: not during a session; downloading after it");
            // `update_phase` makes it due again when the session is over.
            return now + CALENDAR_EVERY;
        }
        match fetch_calendar(unix) {
            Ok(sessions) => {
                self.schedule.replace(sessions, unix);
                log_calendar(self.schedule.calendar.followed(), unix);
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
        let feed_running = self.shared.live.feed_running();
        // From the page's settings, through the render loop's snapshot.
        let follow = self.shared.snapshot.lock().unwrap().settings.sessions;
        self.schedule.calendar.follow(follow);
        let update = self.schedule.update_phase(unix, feed_running, self.tx)?;
        self.shared
            .live
            .set_wanted(cfg!(feature = "live-now") || schedule::feed_wanted(update.phase));
        if update.session_over {
            // The download skipped during it, and any session that moved.
            self.due.calendar = now;
            self.calendar_retry.reset();
        }
        // A replay owns the phase, and the countdown goes with it.
        if !cfg!(feature = "player") {
            self.send_next_start(now, unix)?;
        }
        Ok(now + schedule::phase_wait(self.schedule.calendar.sessions(), unix))
    }

    /// Tells the render loop when the next session starts, for the
    /// countdown, each time that changes.
    fn send_next_start(&mut self, now: Instant, unix: i64) -> anyhow::Result<()> {
        let target = schedule::countdown_target(self.schedule.calendar.followed(), unix)
            .map(|s| (s.key, s.start));
        if target == self.countdown_sent {
            return Ok(());
        }
        let start = target.and_then(|(_, start)| instant_at(now, unix, start));
        self.tx
            .send(Input::NextStart(start))
            .context("render loop is gone")?;
        self.countdown_sent = target;
        Ok(())
    }

    /// Fetches one missing result of the weekend shown on the page, once WiFi
    /// and the clock are ready and no session is on (OpenF1 refuses then).
    /// Returns when to look again: at once after a success, as more may be
    /// missing. Never fails: the page works without results.
    fn update_results(&mut self, now: Instant) -> Instant {
        let (Some(unix), true) = (clock::unix_now(), self.wifi_up()) else {
            return now + RESULTS_EVERY;
        };
        let calendar = &self.schedule.calendar;
        if !calendar.may_download(unix, self.shared.live.feed_running()) {
            return now + RESULTS_EVERY;
        }
        let due = weekend::result_due(
            calendar.sessions(),
            unix,
            &self.results.lock().unwrap(),
            &self.results_given_up,
        );
        let Some(ResultDue { key, last_try }) = due else {
            return now + RESULTS_EVERY;
        };
        match fetch_result(key, self.heartbeat) {
            Ok(Some(result)) => {
                log::info!(
                    "result: session {key}: {} (#{})",
                    result.name,
                    result.driver
                );
                let mut results = self.results.lock().unwrap();
                // Only this calendar's sessions, so the map can't grow all season.
                results.retain(|k, _| calendar.sessions().iter().any(|s| s.key == *k));
                results.insert(key, result);
                now
            }
            Ok(None) if last_try => {
                log::info!("result: none for session {key}; not asking again");
                self.results_given_up.insert(key);
                now
            }
            Ok(None) => {
                log::info!("result: session {key} not published yet");
                now + RESULTS_EVERY
            }
            Err(e) => {
                log::warn!("result: {e:#}");
                now + RESULTS_EVERY
            }
        }
    }

    /// The OpenF1 winner lookup, once WiFi and the clock are ready: if a
    /// race is in its winner window and its winner hasn't been sent yet,
    /// looks it up and sends `RaceEvent::Winner`. Failures only log: it's
    /// tried again a minute later.
    fn look_for_winner(&mut self) -> anyhow::Result<()> {
        let (Some(now_unix), true) = (clock::unix_now(), self.wifi_up()) else {
            return Ok(());
        };
        let lookup = schedule::winner_lookup(
            self.schedule.calendar.followed(),
            now_unix,
            self.winner_sent,
            self.shared.live.winner_shown(),
            self.shared.live.feed_running(),
        );
        let key = match lookup {
            WinnerLookup::Nothing => return Ok(()),
            WinnerLookup::ShownByFeed(key) => {
                log::info!("winner: session {key} already shown from the live feed");
                self.winner_sent = Some(key);
                return Ok(());
            }
            WinnerLookup::Fetch(key) => key,
        };

        // Without a team colour the result is `None`: better to try again
        // later than to guess one.
        match fetch_result(key, self.heartbeat) {
            Ok(Some(winner)) => {
                log::info!(
                    "winner: {} #{} ({}), session {key}",
                    winner.name,
                    winner.driver,
                    winner.color
                );
                self.tx
                    .send(Input::Race {
                        event: RaceEvent::Winner {
                            driver: winner.driver,
                            team_color: winner.color,
                        },
                        received: Instant::now(),
                    })
                    .context("render loop is gone")?;
                self.winner_sent = Some(key);
            }
            Ok(None) => log::info!(
                "winner: session {key} not published yet; retrying in {} s",
                WINNER_EVERY.as_secs()
            ),
            Err(e) => log::warn!("winner: {e:#}; retrying in {} s", WINNER_EVERY.as_secs()),
        }
        Ok(())
    }
}

/// P1 of `session_key` with their short name and team colour; `None` until
/// OpenF1 has published the result.
fn fetch_result(session_key: u32, heartbeat: &Heartbeat) -> anyhow::Result<Option<SessionResult>> {
    let results =
        http::get_json::<Vec<ResultDto>>(&openf1::results_url(session_key))?.unwrap_or_default();
    let Some(driver) = openf1::winner(&results) else {
        return Ok(None);
    };
    heartbeat.beat(Instant::now());
    let drivers = http::get_json::<Vec<DriverDto>>(&openf1::driver_url(session_key, driver))?
        .unwrap_or_default();
    Ok(drivers.first().and_then(openf1::session_result))
}

/// Downloads the sessions around now (see `openf1::sessions_url`), cancelled
/// ones included.
fn fetch_calendar(now_unix: i64) -> anyhow::Result<Vec<Session>> {
    let url = openf1::sessions_url(now_unix);
    log::info!("calendar: GET {url}");
    // `None` is OpenF1's 404 for "no sessions", e.g. in the winter break.
    let dtos = http::get_json::<Vec<SessionDto>>(&url)?.unwrap_or_default();
    Ok(openf1::all_sessions(dtos))
}

fn log_calendar(sessions: &[Session], now_unix: i64) {
    log::info!(
        "calendar: {} sessions in the next {CALENDAR_DAYS} days",
        sessions.len()
    );
    log_next(sessions, now_unix);
}

/// `phase: Live (Singapore Race, session 11388)`, then the next session.
fn log_phase(sessions: &[Session], now_unix: i64, feed_running: bool) {
    match schedule::current_session(sessions, now_unix, feed_running) {
        Some((phase, s)) => log::info!("phase: {phase:?} ({}, session {})", title(s), s.key),
        None => log::info!("phase: Idle"),
    }
    log_next(sessions, now_unix);
}

/// The next session and a countdown to it.
fn log_next(sessions: &[Session], now_unix: i64) {
    match schedule::next_session(sessions, now_unix) {
        Some(next) => log::info!(
            "next: {} (session {}) in {}",
            title(next),
            next.key,
            in_words(next.start - now_unix)
        ),
        None => log::info!("next: no session coming up"),
    }
}

/// `Singapore Race`, or just `Race` while the country is unknown.
fn title(session: &Session) -> String {
    if session.country.is_empty() {
        session.label().to_string()
    } else {
        format!("{} {}", session.country, session.label())
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
