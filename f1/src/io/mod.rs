//! Everything that touches the chip or the network; the decisions are in
//! f1-core.

pub mod clock;
pub mod http;
pub mod led;
// A replay build plays its own events and never connects to the live feed.
#[cfg_attr(feature = "player", allow(dead_code))]
pub mod live;
pub mod mdns;
pub mod ota;
pub mod storage;
pub mod system;
pub mod token_store;
pub mod weblog;
pub mod wifi;
#[cfg_attr(feature = "player", allow(dead_code))]
pub mod ws;
