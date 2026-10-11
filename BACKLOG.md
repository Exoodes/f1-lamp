# F1 Lightbox backlog

Ideas for after the course, in no particular order of size.

## Wanted

- **Fastest lap when the holder beats their own time.** Missed today: the
  lamp counts a fastest lap only when `PersonalBestLapTime` comes with
  `Position: 1` in the same message, and a driver already P1 gets no
  `Position` (it didn't change). Race Singapore 2026-10-11: HAM lap 22
  1:37.170 shown, lap 23 `{"Lap":23,"Value":"1:37.075"}` (capture offset
  01:01:54.671) missed. Fix in `tracker.rs` `fastest_laps`: remember who holds
  the session best; a new time from that driver is a fastest lap too. Small;
  test with today's messages.
- **Overtakes from `DriverRaceInfo`.** The live feed never sends
  `OvertakeSeries`: not in FP1, not in today's race, not even to a probe with
  `--extra` and the F1TV token (the other token-only streams arrived). It
  seems to be archive-only, so overtakes as built can't work live.
  `DriverRaceInfo` (token only) arrived all race and marks each pass:
  `{"87":{"Position":"11","OvertakeState":2},"31":{"Position":"12","OvertakeState":0}}`
  (2 = gained the place, 0 = lost it, 1 = reset a few seconds later). A car
  that pits is "passed" by several cars in a row (#5 at 00:35:29 dropped from
  17 to 20 in a second), so stops must be filtered out (in pit lane, or
  `PitLaneTimeCollection`). The biggest of these.
- **The race log shows everything.** Every event, not only what the lamp
  shows: pit stops when switched off, drivers not followed, switched-off
  effects; still in step with the TV. Lines the lamp didn't show dimmed.
  Today logging is in `controller.rs` `apply_race_event` after `shows()`.
- **Page requests with a timeout.** The saved log of today's race stops at
  14:57, an hour before the end, with no error: likely one `/api/log`
  request hung, and `logBusy` stays true forever so the page never asks
  again (4 sockets shared by all the page's polling; a dropped connection
  never answers). A timeout (`AbortSignal.timeout`) on the page's fetches.
  To ask: did the Log tab visibly stop?
- **Today's race as test data**, from the probe capture
  `f1-probe/captures/2026-10-11_13-40-12`, trimmed to the streams the lamp
  uses (TimingStats 236 KB; only the needed lines). Covers the fastest-lap
  fix; with `DriverRaceInfo`, the overtakes.
- **A "winner" for practice.** E.g. the team colour of the driver with the
  fastest lap when the session ends, like pole after qualifying.
- **Rework the effects.** Some are too fast (seen again in sprint
  qualifying, Singapore 2026-10-09), and some look too much alike.
- **A new chequered flag.** The current one (one lit pixel in four, swapping
  every 500 ms) doesn't look good and is too short (10 s).
- **Fastest driver of each qualifying part.** At the end of Q1 and Q2 (and
  SQ1/SQ2) show the team colour of the part's quickest driver, like pole
  after Q3. The leader is only certain once the last cars finish their laps
  after the flag (Singapore SQ1: a new P1 48 s after the flag). Pole
  already waits for that (`tracker.rs`: `start_last_laps`,
  `pole_after_last_laps`, at most `LAST_LAPS_MAX` after the flag); the same
  check can end Q1 and Q2. To decide: the same look as the winner or one of
  its own, and whether the winner's on/off setting covers it.
- **The panel animated like the lamp.** Today it only takes the shown
  colour. `GET /api/state` could also send which effect shows, its settings
  and how long ago it started (e.g. breathe, yellow, 3 s a breath, started
  1.2 s ago), and the page play it with CSS/JS: the safety car's chase could
  run across the panel's dot grid. About 1-2 h. The page asks every 2 s, so
  short flashes (pit stop, fastest lap: under a second) would often be
  missed; to show those too, either ask a small dedicated endpoint every
  ~250 ms, or have the lamp keep the effects started in the last few seconds
  for the page to play a little late (about half a day with this part).
- **The page's countdown in TV time.** "This weekend" counts to the
  scheduled start and says "live" then; the lamp's countdown effect runs
  until the start plus the TV delay. Adding the delay on the page (it's in
  the settings the page already has) would make both reach zero together.
  Small, page only.
- **Check the page's script in `check.ps1`.** A syntax error in
  `f1/web/index.html` stops the whole page (race day 2026-10-11: a line break
  inside a string, "connecting…" forever), and nothing caught it. Needs Node
  (`node --check` on the extracted script) or a small standalone checker.

## Later

- **Home Assistant over MQTT.** Written by me: the lamp's state out,
  the web page's commands in.

## Maybe

- **Retirement of a followed driver.** A slow fade in the team colour.
- **Penalties for followed drivers.** Race control names the car
  ("10 SECOND TIME PENALTY FOR CAR 5"); a short flash.
- **Rain.** `WeatherData` (public) has `Rainfall`; a blue shimmer when it
  starts raining.
- **Overtakes without an F1TV token**, from position swaps in `DriverList`
  minus pit stops (`PitLaneTimeCollection`). Only if `DriverRaceInfo` (see
  Wanted) isn't enough.

## Open questions

- **How to sync the TV delay easily, also before a race?** Ideas, most
  useful first:
  - Lap times as the TV shows them: "VER 1:32.456" on the page at the
    delayed moment, when the TV's graphic pops up; many a minute in practice
    and qualifying.
  - Race control messages at TV time ("DRS ENABLED", investigations), to
    compare with the broadcast's banners; less exact, the broadcast adds its
    own lag. Part of the race log.
  - A "TV shows it now" button: press it when the TV shows the last feed
    event (a flag, the chequered flag); the lamp sets the delay to the time
    since the feed reported it, minus a little for reaction time.
  - Tap along: press each time the TV's lap counter or clock changes; the
    lamp averages the taps and drops outliers. An upgrade of the button.
  - A short beep from the page at the delayed moment of each lap change or
    flag: easier to judge by ear while watching.
  - Calibration mode: for a few minutes the lamp gives a tiny flash at each
    leader lap change or sector, to match against the TV next to it.
  - A delay per stream ("F1 TV", "TV broadcast"), switched with one tap; this
    stream was steady at 43-44 s (FP1, qualifying Singapore 2026).
  - Suggest from history: the median of the last few delays found.
  - A delay change applies to events arriving afterwards only; retiming the
    events already waiting would make a correction act at once (matters
    just before the start lights).
  - A fixed 0.2-0.3 s lead: a lamp a moment early feels in sync, a late one
    laggy.
  - Streams drift (pausing, buffering): only a quick correction helps.
  - Probably not worth it: reading F1 TV's video timestamps in the browser
    (exact, but research and F1 TV only); a light sensor or microphone on
    the lamp (too complex).
  - Before a race the feed has little the broadcast shows: "GREEN LIGHT -
    PIT EXIT OPEN" about 40 minutes before, "PIT EXIT CLOSED" later, cars
    leaving the pits (`InPit`). The feed has no countdown to the start, and
    SessionStatus `Started` comes at lights out, not at the formation lap
    (Monza 2026 race: lap 2 began 88 s after it, a normal first lap). So the
    start lights depend on the delay set beforehand.

## Seen in sessions

- **Qualifying knockouts come late:** the feed marks the knocked-out drivers
  (`TimingData` `KnockedOut`) only when the next part is announced, about 6
  minutes after the chequered flag (Monza 2026). The leader, too, can
  change after the flag: at Monza a new leader came 70 s after Q3's clock
  ran out.

- **Race Singapore, 2026-10-11:** TV delay about 46 s (HAM's fastest lap:
  feed 14:40:31, lamp 14:41:17). Lights out in the feed 14:03:41. No
  overtakes (see Wanted), one fastest lap missed (see Wanted). VER, PIA, HAD
  set fastest laps on laps 35-36 within 3 minutes (feed 15:01:31, 15:01:57,
  15:04:33), all proper `Position: 1` messages; not confirmed on the lamp
  because the saved log stops at 14:57: check the video at about 15:02:17,
  15:02:43, 15:05:19. The race log repeated "Session started" every 2 s
  (paging bug, fixed after the race, not yet flashed). Heap lowest since
  boot 41,788 B.
- **Sprint qualifying Singapore, 2026-10-09:** pole came too early, right
  after the chequered flag, while many drivers were still on their flying
  laps (that build was older than 28ce6c2). 28ce6c2 made it wait for
  `Finalised`, 18 min 51 s after SQ3's flag here; now it waits until every
  car on track has finished its lap, at most 2 minutes (88 s in this
  session). Also seen: the lamp went white during SQ1's red flag while the
  session was `Inactive` before the restart; fixed, it stays red now.
- **Qualifying Singapore, 2026-10-10:** TV delay 44 s all session (feed
  green 15:30:02, lamp 15:30:46; each chequered flag the same). Pole (#3)
  came 2 min after Q3's flag, at the cap: the lamp had reconnected at
  16:25:20, mid-Q3, so it knew nothing about cars already in the garage
  (the timing snapshot is too large and is dropped). To check against the
  archive once it's up: when every car was done without the reconnect.
  No red flag, so that fix is still untested live. Two WiFi drops an hour
  apart (15:22, 16:23; the router's fault), ~1 min 45 s without the feed
  each. Lowest heap since boot 46,576 B, reached before 15:21 (earlier
  measurements ~120 KB); worth finding what took it. Largest block right
  after the mid-session reconnect: 32,768 B.
- **FP1 Singapore, 2026-10-09:** the TV delay is about 43 s. The logs and
  everything else worked.
- **Pit stops in practice** mostly don't show, and that's the feed: of 78
  `PitLaneTimeCollection` messages only 19 had a `Duration`, the rest
  `"Duration":""` (e.g. a stop to change the setup). The lamp needs the
  duration to tell a stop from a red-flag wait.
- **No overtakes in practice:** the feed sends none. With the F1TV token, both
  the lamp and a probe run with `--extra` got no `OvertakeSeries` data in FP1;
  overtakes come only in races (and sprints).

## Not planned

- The BOOT button: it's inside the lamp.
- Updating firmware from the web page: `cargo ota` covers it.
- The lamp downloading races itself: replays stay embedded at build time.
