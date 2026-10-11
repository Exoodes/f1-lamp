# F1 Lightbox backlog

Ideas for after the course, in no particular order of size.

## Wanted

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
  minus pit stops (`PitLaneTimeCollection`). Only if `OvertakeSeries` with a
  token isn't enough.

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
