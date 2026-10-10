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
- **Rework the effects.** Some are too fast, and some look too much alike.
  The chequered flag doesn't look good and is too short (10 s).
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

## Seen in sessions

- **Qualifying knockouts come late:** the feed marks the knocked-out drivers
  (`TimingData` `KnockedOut`) only when the next part is announced, about 6
  minutes after the chequered flag (Monza 2026). Pole, too, is only certain
  at `Finalised`: at Monza a new leader came 70 s after Q3's clock ran out.

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
