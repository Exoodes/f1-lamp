//! Everything that touches the chip or the network; the decisions are in
//! f1-core.

pub mod clock;
pub mod http;
pub mod led;
pub mod live;
pub mod mdns;
pub mod ota;
pub mod racelog;
pub mod storage;
pub mod system;
pub mod token_store;
pub mod weblog;
pub mod wifi;
pub mod ws;
