# F1 Lightbox backlog

Ideas for after the course, in no particular order of size.

## Wanted

- **Full qualifying support.** The chequered flag at the end of Q1, Q2 and Q3,
  and a signal when a followed driver is knocked out. (Today qualifying shows
  flags, but treats it as one session.)
- **Last lap.** A short white flash when the final lap starts, like a bell.
  `LapCount` (public) has `CurrentLap` and `TotalLaps`.
- **Weekend schedule on the page.** The upcoming sessions with their times and
  a countdown; the lamp already has them from OpenF1.

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

## Not planned

- The BOOT button: it's inside the lamp.
- Updating firmware from the web page: `cargo ota` covers it.
- The lamp downloading races itself: replays stay embedded at build time.
