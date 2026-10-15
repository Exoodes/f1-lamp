# F1 Lightbox backlog

- **Overtakes from `DriverRaceInfo`.** The live feed never sends
  `OvertakeSeries` (not even with the F1TV token), so overtakes as built
  can't work live. `DriverRaceInfo` (token only) arrives all race and marks
  each pass:
  `{"87":{"Position":"11","OvertakeState":2},"31":{"Position":"12","OvertakeState":0}}`
  (2 = gained the place, 0 = lost it, 1 = reset a few seconds later). A car
  that pits is "passed" by several cars in a row (#5 at 00:35:29 dropped from
  17 to 20 in a second), so stops must be filtered out (in pit lane, or
  `PitLaneTimeCollection`). Test data: add the capture's `DriverRaceInfo`
  (`f1-probe/captures/2026-10-11_13-40-12`) to the Singapore race fixture
  (`f1-core/tests/data/2026-singapore/race`, `tests/singapore_race.rs`).
  The race log then fills up: all drivers' overtakes are about 300 a race,
  past its 120 lines. A line costs about 100 B of heap (time and text as
  strings); store the event and the time as a number instead (about 16 B),
  write the text when the page asks, and send a page that just opened only
  the newest lines.
- **Check `esp_websocket_client` 1.8.0 on the lamp:** reconnects in a
  session (or a `live-now` build) with no reboot.
- **Read the WebSocket queue's heap in a session:** the lamp logs
  `live: after a dropped frame, heap … B free, largest block … B` on each
  connection. Only if the heap is low there: a bounded `sync_channel`
  (blocking send slows down TCP), or drop the chunks while the splitter
  skips a frame.
- **Read the WebSocket task's stack after a session:** the last figure of
  the `heap:` line (6 KB, it does the TLS handshake). Raise
  `config::stack::WS_TASK` if little is left.
- **Check the host check on the GETs:** `curl -i -H "Host: x.example"
  http://f1-lightbox.local/api/state` answers 403, and the page still works.
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
  - A delay per stream ("F1 TV", "TV broadcast"), switched with one tap.
  - Suggest from history: the median of the last few delays found.
  - A delay change applies to events arriving afterwards only; retiming the
    events already waiting would make a correction act at once (matters
    just before the start lights).
  - A fixed 0.2-0.3 s lead: a lamp a moment early feels in sync, a late one
    laggy.
  - Before a race the feed has little the broadcast shows ("GREEN LIGHT -
    PIT EXIT OPEN" about 40 minutes before, cars leaving the pits), and
    `Started` comes at lights out, so the start lights depend on the delay
    set beforehand.
