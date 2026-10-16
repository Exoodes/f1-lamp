# F1 Lightbox backlog

- **WiFi's first association is rejected on every boot.** Since the USB
  flash of 2026-10-15 the router answers the lamp's first association within
  about 10 ms (`wifi:state: assoc -> init (6c0)`, once `2c0`); the retry
  connects. Last weekend the lamp was online after one or two blue breaths,
  now after several (online at ~16 s with the 8 s wait of 460bd7d, ~23 s
  before it). Not the flash itself (Ctrl+R does it too) and not USB power
  (the adapter does it too). `io/wifi.rs` didn't change before 460bd7d;
  lower-level changes since last weekend: 6994a37 (flash size 4 MB, 1 KB
  request headers), 325660a (`esp_websocket_client` 1.8), 6f7f895 (main
  stack 12 KB), and the new bootloader that USB flash installed. Next:
  bisect with `cargo ota` from a build of last weekend, power-cycling and
  timing `wifi up` in the log at each step.
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
