# F1 Lightbox

A lamp that follows Formula 1 sessions live. It turns yellow when the TV
shows a yellow flag, runs the start lights with the real start, shows the
safety car, VSC and red flag, waves the chequered flag and ends a race in the
winner's team colour. Between sessions it's a normal lamp.

It reads F1's own live timing feed (the one behind the live timing on
formula1.com), runs on a Seeed XIAO ESP32-C3 with 23 WS2812 LEDs,
and is written in Rust. It was built step by step following *F1 Lightbox in
Rust: the course*, with some extras of its own.

## What the lamp shows

| Event | Lamp |
|---|---|
| Countdown | white breathing that speeds up, from one breath in 4 s to one a second, for the last 10 minutes before a session, until the TV shows the start (scheduled start plus the TV delay); can be switched off |
| Green flag | green for a few seconds (setting), then the lamp colour again |
| Yellow | solid yellow |
| Double yellow | blinking yellow |
| Safety car | a yellow light chasing around the ring |
| Virtual safety car | slowly breathing yellow |
| VSC ending | faster breathing yellow, about 15 s before green |
| Red flag | red, also through the whole stoppage |
| Start lights | five red lights going out with the TV (needs a TV delay of at least 6 s) |
| Last lap | two white flashes when the leader starts a race's last lap |
| Chequered flag | a white pattern (one lit LED, three dark) that jumps back and forth, for 10 s, at the end of every session; in qualifying after Q1 and Q2 too |
| Winner | a chase in the winner's team colour, after the chequered flag; in qualifying the fastest driver's after each part (pole's after the last), and after practice the fastest driver's (once every car has finished its last lap, at most 2 minutes after the flag) |
| Fastest lap | three purple flashes, for every new session best |
| Knocked out | the team colour fading out over 3 s (followed drivers, after Q1 and Q2; the feed says so about 6 minutes after the part's chequered flag) |
| Pit stop | two flashes in the team colour (followed drivers) |
| Overtake | one flash in the team colour (followed drivers, needs F1TV) |
| No network | breathing blue (WiFi connecting) or blinking red (the live feed failed 3 times in a row): after a boot until the lamp is first online, then only around a session |

Practice, qualifying, sprint and race are all followed; each kind can be
switched off on the page. A TV delay setting holds everything back so the
lamp changes when your TV does, not when the feed does.

## Hardware

- Seeed Studio XIAO ESP32-C3 (4 MB flash).
- 23 WS2812 LEDs in three chained strips (11 + 7 + 5). Data from GPIO2 (the
  XIAO's pad D0) through a 470 Ω resistor.
- Power: a 5 V / 2 A adapter into a USB-C breakout, which feeds the LEDs'
  +5 V directly and the XIAO's 5 V pin in parallel. All LEDs at full white
  draw about 1.4 A, the XIAO up to about 0.35 A with WiFi: within the 2 A.
  The firmware doesn't limit the brightness.
- Flash over USB only through the XIAO's own USB cable, never with the
  adapter connected at the same time. On that cable the LEDs draw through
  the PC's port (0.5-0.9 A), so keep the brightness low while flashing, or
  update over WiFi with `cargo ota`.

## The repository

```
f1-core/    All the logic, no hardware: parsing the feed, the flag state
            machine, effects, settings, the schedule, the replay. Builds and
            tests on Windows (`#![forbid(unsafe_code)]`).
f1/         The ESP-IDF firmware: WiFi, the LEDs, the live connection, the web
            page, updates over WiFi. Uses f1-core for every decision.
f1-probe/   A PC tool that talks to the live feed and saves what arrives as
            .jsonStream files (for debugging and new test data).
BACKLOG.md  Ideas for later.
```

The firmware is a set of threads that only send `Input` messages to one
render loop: the network thread (WiFi, clock, OpenF1 calendar and winner; it
also starts the web server, which runs on its own task), the live-feed
thread, and in replay builds the replay. The render loop owns the
`Controller` (f1-core), which decides what the 23 LEDs show 50 times a
second. It runs at a higher priority than the network work, so a TLS
handshake doesn't stall the LEDs, and once a second it checks the threads:
one that has ended or hasn't reported for 3 minutes restarts the chip.
`f1/src/config.rs` lists the threads' stacks, the LED settings and the flash
keys.

## Setting up

You need:

- Rust with `rustup`. `f1/rust-toolchain.toml` pins the nightly and its
  `rust-src`, and rustup installs them on the first build. The ESP32-C3 is
  RISC-V, so `espup` (the Xtensa toolchain) isn't needed.
- `cargo install ldproxy espflash` (espflash 4).
- git and Python 3: the first build downloads ESP-IDF v5.2.8 and builds it,
  which takes a while; later builds reuse it.

Create `f1/cfg.toml` (it's in `.gitignore`, keep it out of git):

```toml
[f1]
wifi_ssid = "your network"
wifi_psk = "your password"
# Updates over WiFi need this key (at least 16 visible characters, no quotes).
# `cargo ota` reads it from here; nothing else is accepted by /api/ota.
# It's never sent: uploads only prove it (see below).
ota_key = "some long random text"

# Only for --features replay: the session to play.
[replay]
dir = "C:/path/to/f1-data/2026-italy/race"
```

## Building and flashing

Always release builds: a debug build doesn't fit an app slot.

```
cd f1
cargo run --release          # over USB: flash and open the serial monitor
cargo ota                    # over WiFi: build, upload, wait for the lamp
```

The first USB flash writes the partition table (`partitions.csv`): two app
slots for updates over WiFi. Settings, the F1TV token and the cached
schedule live in NVS, which no flash or update touches. A USB flash always
writes the first slot and makes it the one to boot, so it's also the way
back when updates over WiFi don't work any more.

The WiFi name and password are built into the firmware from `cfg.toml`. To
change them, edit `cfg.toml` and send a new build: with `cargo ota` while
the lamp can still reach your network, otherwise over USB.

`cargo ota` is an alias (`f1/.cargo/config.toml`) that builds like
`cargo run --release` and hands the result to `ota.ps1`, which uploads it to
`f1-lightbox.local`. The `ota_key` from `cfg.toml` never leaves the PC: the
lamp hands out a one-time nonce, and the upload carries
HMAC-SHA256(key, nonce + SHA-256(image)), which the lamp checks before the new
firmware can boot, and HMAC-SHA256(key, nonce + "start" + the size), which it
checks before erasing the spare slot (the previous firmware). A recorded upload
can't be sent again, and the lamp refuses an upload without valid tags. A new key takes effect with the firmware that carries it,
so the upload that brings a changed key still needs the old one: after changing
`ota_key`, flash once over USB. If the name doesn't resolve, point it at the lamp's IP:

```powershell
$env:F1_LAMP = "192.168.1.53"; cargo ota; Remove-Item Env:F1_LAMP
```

The router may give the lamp a new address after it restarts; a DHCP
reservation in the router keeps it fixed.

A new firmware runs on trial: if it restarts three times without its web
server running for a minute, the lamp switches back to the previous one on
its own. A trial boot that isn't there 3 minutes after starting (e.g. it never
gets WiFi) restarts itself, so that counts too: such a firmware is gone after
about 10 minutes. The same happens if the WiFi itself is down for that long
right after an update; then just send the update again.

### Feature builds

Any of these work with both `cargo run --release` and `cargo ota`:

| Feature | What it does |
|---|---|
| `replay` | Plays the session in `[replay] dir` (or `$env:F1_REPLAY_DIR`) instead of the live feed. Controlled from the page: play, pause, speed 1x to 200x, jump. |
| `showcase` | Plays a hand-written 2.5-minute session with every flag, a pit window in all 11 team colours, fastest laps, overtakes and a winner. |
| `live-now` | Connects to the live feed right away, whatever the schedule says: for testing between sessions. |
| `crash-test` | A firmware that crashes 10 s after every boot, to test the update fallback. Only send it with `cargo ota`. |

Replay data comes from F1's public archive
(`https://livetiming.formula1.com/static/<year>/Index.json`). The build keeps
only the lines the lamp uses, so a whole race fits in the firmware.

## The web page

`http://f1-lightbox.local` (same network only):

- what the lamp shows, the session phase and the network state;
- the race log: every race event in words ("Yellow flag", "VER in the
  pits", "Fastest lap: NOR"), with its time, in step with your TV; events the
  lamp didn't show (switched off, a driver not followed) are dimmed;
- TV sync: the session clock and the lap as your TV should show them with
  the delay set, and buttons to move the delay by 0.1, 0.5 or 1 second,
  until they change together with the TV's;
- this race weekend: its sessions in your time zone, a countdown to the next
  one (in TV time: it reaches zero when your TV shows the start, with the
  lamp's), which are live, which moved (and from when) or were cancelled,
  and who was fastest, on pole or won (until a day after the race);
- lamp colour and brightness, night mode (in Central European time), TV
  delay, how long green and the winner show;
- which events show, which kinds of session the lamp follows, and the
  followed drivers (or all of them);
- forcing a flag or a colour by hand, and trying any effect once;
- the F1TV token (see below);
- in replay builds, the replay controls;
- a live log of the lamp's own messages, with times.

Settings save on their own as you change them.

Every POST to the lamp must carry the header `X-F1-Lamp` (any value), which
the page sends; other websites can't, so they can't change the lamp through a
browser on your network. Scripts must add it too, e.g.
`curl -H "X-F1-Lamp: 1" -d '{"kind":"release"}' http://f1-lightbox.local/api/override`.
POSTs, and GETs of `/api/...`, must also address the lamp as
`f1-lightbox.local`, `f1-lightbox` or its IP address: a name the router
gives it (e.g. `f1-lightbox.fritz.box`) is refused with 403, like any page
that tricks a browser into reaching the lamp under its own domain, so such
a page can't read the log or the token's status either. The page itself
opens under any name.

The page uses these, and scripts can too:

| Request | What it does |
|---|---|
| `GET /api/state` | What the lamp shows: layer and its colour, phase, network, override, settings, and the session clock and lap as the TV shows them (JSON) |
| `GET /api/schedule` | This race weekend: the lamp's time, the country, the circuit and the sessions with their state (`done`, `live`, `coming`), whether they're followed, where a moved one was, whether it's cancelled, and P1 of each finished one |
| `POST /api/settings` | All the settings as JSON: get them from `/api/state`, change, send back |
| `POST /api/override` | `{"kind":"flag","flag":"red"}`, `{"kind":"color","color":"#ff8800"}` or `{"kind":"release"}`; or plays one effect at once: `{"kind":"event","event":"pit_stop","team":"mclaren"}` (`start_lights`, `chequered_flag`, `last_lap`, `fastest_lap`, `pit_stop`, `overtake`, `winner`, `knocked_out`; teams `alpine`, `aston_martin`, `audi`, `cadillac`, `ferrari`, `haas`, `mclaren`, `mercedes`, `racing_bulls`, `red_bull`, `williams`) |
| `GET /api/token`, `POST /api/token` | The F1TV token's state; post a token (plain text) to set it, an empty one to remove it |
| `GET /api/racelog?after=<n>` | The race log's lines from number `n` on, where `n` is the last answer's `next` (all kept lines without it): time, text, colour |
| `GET /api/log?after=<n>` | The log lines after number `n` |
| `POST /api/ota/nonce` | A one-time nonce (hex) for the next upload, valid for 2 minutes |
| `POST /api/ota` | A firmware image, with `X-F1-Nonce` and its tags in `X-F1-Start` and `X-F1-Auth` (see `cargo ota`) |
| `GET /api/replay`, `POST /api/replay` | Replay builds only: the position, and `{"action":"play"}`, `pause`, `{"action":"speed","speed":60}`, `lights_out`, `{"action":"seek","position_ms":4000000}` |

## F1TV (optional)

The flags, start, chequered flag, winner, fastest laps and pit stops all come
from public data. Overtakes (and other account-only streams) need an F1TV
token: log in at formula1.com, copy the value of the `loginSession` cookie
(browser dev tools, Application, Cookies) and paste it on the page. Tokens
expire after a few days; the page shows when, and the lamp falls back to the
public data without one.

The token opens a paid account: it's stored on the lamp only, never shown,
logged or returned by the API. Keep the lamp on your home network.

## Testing

```
cd f1-core
cargo test        # unit tests plus the recorded sessions below
```

The sessions in `f1-core/tests/data/`, each played through the same code as
the lamp:

- the 2026 Italian GP race and qualifying, from F1's archive;
- the 2026 Singapore GP sprint qualifying, from the archive (pole timing,
  a red flag);
- the last half hour of the 2026 Singapore GP FP1, captured live by the
  probe (VSC ending, the chequered flag and P1 in practice);
- the whole 2026 Singapore GP race, captured live by the probe (start
  lights, two VSCs, fastest laps, pit stops, last lap, winner);
- the live feed's answer to `Subscribe` after the 2026 Bahrain GP race, from
  the probe (live data differs from the archive), with SessionData, LapCount
  and ExtrapolatedClock as they stood at the end of the Singapore race;
- the hand-written showcase (`--features showcase`);
- OpenF1's 2026 calendar and the Italian GP race result.

`f1-core` must be tested from its own folder: `f1/.cargo/config.toml` builds
for the ESP32. Before calling a change done, run every check at once (the
same ones the CI in `.github/workflows/ci.yml` runs on each push):

```
powershell -ExecutionPolicy Bypass -File check.ps1          # all
powershell -ExecutionPolicy Bypass -File check.ps1 -Part core
```

It runs a syntax check of the web page's script (`node --check`; needs
Node.js, e.g. `winget install OpenJS.NodeJS.LTS`), formatting, clippy with
warnings as errors, the f1-core tests, the probe, and the release builds (plain, showcase, replay of the archived race)
with a check that each image stays under 95 % of an app slot. Any warning in
their output fails them, also one clippy can't turn into an error (such as an
unused dependency). The CI also runs `cargo audit` on the three lock files.
`cargo fmt --all` from `f1/` fixes formatting in f1 and f1-core.

Watching a live session with the probe (on the PC):

```
cd f1-probe
cargo run                 # until Ctrl+C; saves to captures/<date_time>/
cargo run -- 30 --extra   # 30 minutes, extra streams too
```

With `$env:F1TV_TOKEN` set, the probe sends the token as well.

## Developing

### How data flows

```
F1 live feed ──WebSocket──▶ WebSocket client task ──channel──▶ live thread
(SignalR, wss)              (copies 4 KB chunks)               (LiveSession:
                                                                frames → events)
                                                                     │
OpenF1 ──HTTPS──▶ net thread (WiFi, clock, calendar, phase,          │
                  results, winner fallback) ───────────────────────┐ │
                                                                   ▼ ▼
replay thread (replay builds) ───────────────────────────▶   Input queue
                                                                   │
web page ──POST──▶ web handlers (settings, overrides) ────────────▶│
    ▲                                                              ▼
    │                                         render loop (main): Controller
    │                                         (TV delay, flags, overlays,
    │                                         effects) ──▶ 23 LEDs, 50 / s
    │                                                              │
    └──── GET /api/state, /api/racelog ◀──── snapshot, race log ◀──┘
```

Everything that decides something is in f1-core; the firmware only moves
bytes and `Input`s. The live thread passes events on as they arrive: the TV
delay is applied in the `Controller`, so the page's overrides and "Try an
effect" can skip it.

### Session phases

From the calendar (OpenF1, kept in flash), each session puts the lamp in a
phase (`f1-core/src/schedule.rs`):

| Phase | From | Until | Feed |
|---|---|---|---|
| pre-session | 30 min before the scheduled start | the start | connected |
| live | the scheduled start | 30 min after the scheduled end; while the feed says the session still goes on, up to 45 min more | connected |
| post-session | the end of live | 60 min later; races and sprints only (the winner window: OpenF1 is asked if the feed missed the winner) | closed |
| idle | otherwise | | closed |

When windows of two sessions overlap, live beats pre-session beats
post-session. The countdown runs from 10 minutes before the start until the
TV shows it (the start plus the TV delay).

### The feed's quirks

All seen in real sessions; the code around each has the details.

- Live data carries `"_kf": true` in every stream's first answer; the
  archive doesn't.
- The answer to `Subscribe` is applied silently: it's the state, not news.
  TimingData's (the whole timing screen) and race control's (about 45 KB
  after a race) are too large for the device and skipped, so a car can be
  unknown until its next update.
- Text frames come in 4 KB chunks that can cut through a multi-byte
  character (`–`, `’`), so chunks are put together as bytes.
- The server drops a client that stays quiet for about 30 s, so the lamp
  pings every 15 s.
- `SessionStatus` `Started` comes at lights out, not at the formation lap.
  The feed has no countdown to the start.
- During a red flag the status can go `Aborted`, then `Inactive` until the
  restart: still the stoppage, so the lamp stays red.
- Qualifying and practice are `Finished` when the clock runs out, but cars
  on their last lap still change the order; `Finalised` can come 18 minutes
  later. The knocked-out drivers are marked (`KnockedOut`) only when the
  next part is announced, about 6 minutes after the flag.
- TopThree patches leave out `TeamColour` when it didn't change.
- The fastest lap's holder beating their own time sends a patch with `Lap`
  and no `Position`; a deleted lap sends a slower `Value` without `Lap`.
- Pit stops in practice mostly have an empty `Duration` (a stop to change
  the setup); during a red flag every car waits in the pit lane, so a pit
  lane time over 2 minutes isn't a stop.
- `OvertakeSeries` is never sent live, even with the F1TV token;
  `DriverRaceInfo` is (not used yet).
- OpenF1's free tier answers 401 during live sessions.

### Reading the log

Once a minute (also on the page's log):

- `heap: … B free, … B lowest since boot, largest block … B; stack left:
  main …, net …, live …, WebSocket …; nvs entries used: … of …`. The
  largest block matters more than the free total: a TLS handshake needs
  blocks of several KB, the `Subscribe` answer one of up to `MAX_FRAME`
  (24 KB). Stacks are the least each has had left; `?` until measured.
- `frames: … in 60 s, longest gap … ms`: about 3000 frames is smooth; a gap
  of 100 ms or more is a visible jerk (usually a TLS handshake).

On each connection: `live: subscribed (session …); heap …` and, when race
control's history is skipped, `frame larger than … bytes, dropped` followed
by `live: after a dropped frame, heap …`: the moment the most may wait in the
WebSocket's queue.

### Adding a stream or an event

A new stream from the feed, and an event the lamp shows from it, touch these
places, in this order:

1. `f1-core/src/feed.rs`: the stream's JSON as a struct, a `FeedMessage`
   variant, and a `Stream` variant with its F1 name, `is_relevant` (which
   lines a replay keeps) and `parse`.
2. `f1/build.rs` `REPLAY_FILES` and `f1/src/replay.rs` `FILES`, so replay
   builds embed it.
3. `f1-core/src/live.rs`: the stream lists the lamp subscribes to (an
   account-only stream goes in `ACCOUNT_STREAMS`, which needs the F1TV
   token).
4. `track_state.rs` (flags and the session) or `tracker.rs` (driver events)
   turns its messages into a `RaceEvent` (`input.rs`).
5. `settings.rs`: an `EffectToggles` switch and a line in `Settings::shows`.
6. `theme.rs` `event_overlay`: what the lamp shows.
7. `racelog.rs`: its line in the race log (`text`, `color`).
8. `command.rs` `PreviewEvent`, so "Try an effect" can play it.
9. `f1/web/index.html`: the switch (`EFFECTS`) and the try button.

### Threads, priorities and locks

| Task | Stack | Priority | Does |
|---|---|---|---|
| main (render loop) | 8 KB (`sdkconfig.defaults`) | 6 | the `Controller`, the LEDs, saving settings; checks the threads |
| net | 16 KB | 5 | WiFi, the clock, OpenF1, starts the web server |
| live | 12 KB | 5 | negotiate, the WebSocket, parsing the feed |
| web handlers | 10 KB | 5 | every request, OTA uploads included |
| WebSocket client | 6 KB | 5 | copies the feed's bytes into a channel |
| replay (replay builds) | 8 KB | 5 | plays the embedded session |

The stacks are in `f1/src/config.rs`; the minute `heap:` log line shows how
much stack the main, net and live threads and the WebSocket client's task
(the current connection's, measured when it connects and when it ends) have
had left at least. The
threads only talk to the render loop through the `Input` queue, and the
render loop never waits for the network.

Locks: `http::TLS` lets one TLS handshake run at a time (tens of KB of heap
each); the live thread holds it from connecting until `Subscribe` is
answered, at most `LONGEST_TLS_HOLD`. The rest guard shared state for a
moment each: the snapshot (render loop writes, web reads), the calendar,
moved sessions and results (net writes, web reads), the F1TV token, the race
log and web log buffers, and the OTA nonces and trial. None is held across
network work.

### Recovery over USB

When the lamp can't be reached over WiFi any more (a firmware that doesn't
work, a wrong WiFi password in `cfg.toml`):

1. Unplug the power adapter and connect the XIAO's own USB cable.
2. `cd f1; cargo run --release`. It writes the first app slot and boots it,
   whatever ran before, and opens the serial log. A trial left over from an
   update ends as it boots ("flashed over USB?" in the log).
3. To only watch the log: `espflash monitor`.

If saved data itself is the problem, erasing NVS brings back the default
settings; the F1TV token and the cached calendar are gone too:

```
cd f1
espflash erase-parts --partition-table partitions.csv nvs
```

### A capture into a test fixture

The probe saves everything the feed sends to `f1-probe/captures/<date_time>/`.
To make a test of it (as `f1-core/tests/practice.rs`):

1. Copy the streams the lamp uses (those in `f1/build.rs` `REPLAY_FILES`, and
   `ExtrapolatedClock`) to `f1-core/tests/data/<year>-<place>/<session>/`,
   each trimmed to the lines `Stream::is_relevant` keeps, as `build.rs` does
   for a replay build. `CarData.z` and `Position.z` aren't needed.
2. In the test, embed them with `include_str!` and play them through
   `Replayer` with a fake clock (see `practice.rs`). Print the inputs once to
   see what the session gives, then assert what matters.

A capture also plays on the lamp as it is: point `$env:F1_REPLAY_DIR` at its
folder and build with `--features replay`.

### The NVS budget

NVS is 24 KB: six pages of 126 entries of 32 bytes, one page always kept
free, so about 630 entries. A string or blob takes one entry per 32 bytes plus
one, and a rewrite needs room for the new copy before the old one goes.

| Key (`config::nvs`) | Size |
|---|---|
| `settings` | JSON, at most 1 KB (`settings::MAX_JSON`) |
| `schedule` | JSON, at most 3 KB (`schedule::MAX_JSON`) |
| `f1tv_token` | at most 3 KB (`token::MAX_LEN`) |
| `ota_trial`, `ota_trial_img`, `ota_fell_back` | a few bytes |

So the lamp's own data takes at most about 240 entries, plus what the WiFi
driver keeps in its own namespace. The `heap:` log line shows the entries in
use.

## Data sources

- **F1 live timing** (`livetiming.formula1.com`, SignalR over a WebSocket):
  flags, session status, race control, driver list, top three, pit lane,
  timing stats, overtakes, qualifying parts (session data), knockouts
  (timing data), the race's laps (lap count: the last lap and the page's TV
  lap) and the session clock (extrapolated clock: the page's TV clock). Unofficial and undocumented; it changes without
  notice. When the lamp stops reacting on a race weekend, run the probe on
  the PC first.
- **OpenF1** (`api.openf1.org`): the session calendar, each session's P1
  for the page, and the winner as a fallback after a reboot. Its free tier
  is closed during live sessions, so the calendar is fetched ahead, kept in
  flash, and not fetched from 30 minutes before a session until it ends
  (then once straight away); results wait for the session to end too.

This is a personal project, not affiliated with Formula 1. F1, Formula 1 and
F1TV are trademarks of Formula One Licensing B.V.
