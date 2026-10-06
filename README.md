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
| Green flag | green for a few seconds (setting), then the lamp colour again |
| Yellow | solid yellow |
| Double yellow | blinking yellow |
| Safety car | a yellow light chasing around the ring |
| Virtual safety car | slowly breathing yellow |
| Red flag | red, also through the whole stoppage |
| Start lights | five red lights going out with the TV (needs a TV delay of at least 6 s) |
| Chequered flag | a white pattern (one lit LED, three dark) that jumps back and forth, for 10 s |
| Winner | a chase in the winner's team colour |
| Fastest lap | three purple flashes |
| Pit stop | two flashes in the team colour (followed drivers) |
| Overtake | one flash in the team colour (followed drivers, needs F1TV) |
| No network | breathing blue (WiFi connecting) or blinking red (the live feed failed 3 times in a row), only around a session |

Practice, qualifying, sprint and race are all followed. A TV delay setting
holds everything back so the lamp changes when your TV does, not when the
feed does.

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
- git and Python 3: the first build downloads ESP-IDF v5.2 and builds it,
  which takes a while; later builds reuse it.

Create `f1/cfg.toml` (it's in `.gitignore`, keep it out of git):

```toml
[f1]
wifi_ssid = "your network"
wifi_psk = "your password"
# Updates over WiFi need this key (at least 16 visible characters, no quotes).
# `cargo ota` reads it from here; nothing else is accepted by /api/ota.
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
`f1-lightbox.local` with the `ota_key` from `cfg.toml`; the lamp refuses an
upload without it. A new key takes effect with the firmware that carries it,
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
- lamp colour and brightness, night mode (in Central European time), TV
  delay, how long green and the winner show;
- which events show, and the followed drivers (or all of them);
- forcing a flag or a colour by hand;
- the F1TV token (see below);
- in replay builds, the replay controls;
- a live log of the lamp's own messages, with times.

Settings save on their own as you change them.

Every POST to the lamp must carry the header `X-F1-Lamp` (any value), which
the page sends; other websites can't, so they can't change the lamp through a
browser on your network. Scripts must add it too, e.g.
`curl -H "X-F1-Lamp: 1" -d '{"kind":"release"}' http://f1-lightbox.local/api/override`.

The page uses these, and scripts can too:

| Request | What it does |
|---|---|
| `GET /api/state` | What the lamp shows: layer, phase, network, override, settings (JSON) |
| `POST /api/settings` | All the settings as JSON: get them from `/api/state`, change, send back |
| `POST /api/override` | `{"kind":"flag","flag":"red"}`, `{"kind":"color","color":"#ff8800"}` or `{"kind":"release"}` |
| `GET /api/token`, `POST /api/token` | The F1TV token's state; post a token (plain text) to set it, an empty one to remove it |
| `GET /api/log?after=<n>` | The log lines after number `n` |
| `POST /api/ota` | A firmware image; also needs `X-F1-Key` (see `cargo ota`) |
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
cargo test        # unit tests plus the archived 2026 Italian GP and the showcase
```

`f1-core` must be tested from its own folder: `f1/.cargo/config.toml` builds
for the ESP32. Before calling a change done, run every check at once (the
same ones the CI in `.github/workflows/ci.yml` runs on each push):

```
powershell -ExecutionPolicy Bypass -File check.ps1          # all
powershell -ExecutionPolicy Bypass -File check.ps1 -Part core
```

It runs formatting, clippy with warnings as errors, the f1-core tests, the
probe, and the release builds (plain, showcase, replay of the archived race)
with a check that each image stays under 95 % of an app slot.
`cargo fmt --all` from `f1/` fixes formatting in f1 and f1-core.

Watching a live session with the probe (on the PC):

```
cd f1-probe
cargo run                 # until Ctrl+C; saves to captures/<date_time>/
cargo run -- 30 --extra   # 30 minutes, extra streams too
```

With `$env:F1TV_TOKEN` set, the probe sends the token as well.

## Data sources

- **F1 live timing** (`livetiming.formula1.com`, SignalR over a WebSocket):
  flags, session status, race control, driver list, top three, pit lane,
  timing stats, overtakes. Unofficial and undocumented; it changes without
  notice. When the lamp stops reacting on a race weekend, run the probe on
  the PC first.
- **OpenF1** (`api.openf1.org`): the session calendar, and the winner as a
  fallback after a reboot. Its free tier is closed during live sessions, so
  the calendar is fetched ahead and kept in flash.

This is a personal project, not affiliated with Formula 1. F1, Formula 1 and
F1TV are trademarks of Formula One Licensing B.V.
