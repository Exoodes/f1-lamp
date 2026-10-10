# F1 Lightbox backlog

Ideas for after the course, in no particular order of size.

## Wanted

- **Last lap.** A short white flash when the final lap starts, like a bell.
  `LapCount` (public) has `CurrentLap` and `TotalLaps`.
- **Weekend schedule on the page.** The upcoming sessions with their times and
  a countdown; the lamp already has them from OpenF1.
- **Event buttons on the page.** Next to the flag overrides: trigger a fastest
  lap, a pit stop or an overtake for any driver (in their team colour), to see
  the effects without waiting for a session.
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
- **VSC ending.** The feed sends it (`TrackStatus` 7, `VSCEnding`, ~15 s
  before green; FP1 Singapore had one), but the lamp treats it like VSC and
  keeps breathing yellow. Give it a look of its own, e.g. faster breathing.
- **No calendar downloads during a session.** In pre-session and live, OpenF1
  answers 401 anyway ("Live F1 session in progress"; every 30 min in FP1),
  and each try costs a TLS handshake while the feed is open. Skip them while
  the feed is wanted and download once right after the session.

## Later

- **Countdown before a session.** E.g. from 10 minutes before the start the
  lamp breathes slowly in white, faster towards the start.
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

- **Race log on the page.** A readable list of what happened, in step with
  the TV (after the delay, when the lamp shows it): "Yellow flag", "NOR in
  the pits", "Fastest lap: VER", "PIA overtook". Names from DriverList
  (`Tla`, `LastName`). Today the page's log has only the raw lines, logged
  when the feed sends them, not when the lamp shows them, e.g.
  `live: FastestLap { driver: 1, .. }`, and keeps only a few dozen lines.
  `OvertakeSeries` names only who overtook; who was passed would have to
  come from position swaps.

## Open questions

- **How to sync the TV delay easily, also before a race?** To think about.
  Ideas so far:
  - A "now" button on the page: press it when the TV shows the last event
    (a flag, pit exit open, the chequered flag); the lamp sets the delay to
    the time since the feed reported it.
  - The session clock in the log as the TV will show it (`ExtrapolatedClock`
    with the delay added): "TV clock 28:00" should appear when the TV's clock
    does. Practice, qualifying and sprint qualifying only; races show laps
    (`LapCount`), which could be logged the same way.
  - Before a race the feed sends "GREEN LIGHT - PIT EXIT OPEN" about 40
    minutes before the start, when the TV shows the cars leaving the pits.
  - The TV's countdown to a race start is only a graphic: the feed has no
    countdown (checked on the Singapore sprint 2026-10-10, whose start was
    delayed). The nearest thing is the start of the formation lap:
    SessionStatus `Started` and `ExtrapolatedClock` starting to count down
    from 1:00:00 come together (09:36:31.0 UTC there), when the TV shows the
    cars pulling away. A good moment for the "now" button.

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
