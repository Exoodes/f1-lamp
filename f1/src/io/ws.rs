//! A TLS WebSocket client: a thin wrapper over ESP-IDF's
//! `esp_websocket_client` component.
//!
//! Not the `esp_idf_svc::ws` wrapper: that hands text frames over as `&str`
//! chunk by chunk, and a large frame is cut into chunks of `buffer_size`
//! bytes. A cut through a multi-byte character (F1 sends `–` and `’`) makes
//! that chunk fail UTF-8 and it's lost. Here every chunk arrives as bytes, and
//! the SignalR splitter puts frames together.

use std::{
    ffi::{c_void, CString},
    sync::mpsc::Sender,
    time::Duration,
};

use anyhow::{bail, Context};
use esp_idf_svc::{
    hal::delay::TickType,
    sys::{
        esp, esp_crt_bundle_attach, esp_event_base_t, esp_websocket_client_close,
        esp_websocket_client_config_t, esp_websocket_client_destroy, esp_websocket_client_handle_t,
        esp_websocket_client_init, esp_websocket_client_is_connected,
        esp_websocket_client_send_text, esp_websocket_client_start, esp_websocket_event_data_t,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_ANY,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_CLOSED,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_CONNECTED,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_DATA,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_DISCONNECTED,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_ERROR, esp_websocket_register_events,
    },
};

/// Bytes the client reads at a time; larger frames come in several chunks.
const BUFFER_SIZE: i32 = 4096;
const NETWORK_TIMEOUT_MS: i32 = 10_000;
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// What the client's task reports.
#[derive(Debug)]
pub enum WsEvent {
    Connected,
    /// A piece of a text or binary frame. `frame_len` is the whole frame's
    /// length and `offset` where this piece starts in it.
    Data {
        bytes: Vec<u8>,
        frame_len: usize,
        offset: usize,
    },
    /// The server closed the connection, or it dropped.
    Disconnected,
    Error,
}

pub struct WebSocket {
    handle: esp_websocket_client_handle_t,
    /// Owned here, read by `on_event` on the client's task until `drop`.
    _events: Box<Sender<WsEvent>>,
    /// Kept alive while the client may read them.
    _uri: CString,
    _headers: CString,
}

// SAFETY: the handle is only used through the client's thread-safe API, and
// the boxed sender is `Send`.
unsafe impl Send for WebSocket {}

impl WebSocket {
    /// Starts connecting to `uri` (`wss://...`); events go to `events`.
    /// `headers` are extra request headers, each ending in `\r\n`.
    pub fn connect(uri: &str, headers: &str, events: Sender<WsEvent>) -> anyhow::Result<Self> {
        let uri = CString::new(uri).context("URI contains a NUL byte")?;
        let headers = CString::new(headers).context("headers contain a NUL byte")?;
        let events = Box::new(events);

        let config = esp_websocket_client_config_t {
            uri: uri.as_ptr(),
            headers: headers.as_ptr(),
            crt_bundle_attach: Some(esp_crt_bundle_attach),
            buffer_size: BUFFER_SIZE,
            task_stack: crate::config::stack::WS_TASK,
            network_timeout_ms: NETWORK_TIMEOUT_MS,
            // Reconnecting needs a fresh negotiate, so live.rs does it.
            disable_auto_reconnect: true,
            ..Default::default()
        };

        // SAFETY: `config` and the strings it points to live until the call
        // returns; the client copies what it keeps.
        let handle = unsafe { esp_websocket_client_init(&config) };
        if handle.is_null() {
            bail!("esp_websocket_client_init failed");
        }
        let client = WebSocket {
            handle,
            _events: events,
            _uri: uri,
            _headers: headers,
        };
        // The box's contents don't move when `client` does.
        let arg = std::ptr::from_ref::<Sender<WsEvent>>(&client._events) as *mut c_void;

        // SAFETY: `handle` is valid; `arg` points into `client`, which owns
        // the box and destroys the client (stopping all events) before
        // freeing it.
        esp!(unsafe {
            esp_websocket_register_events(
                handle,
                esp_websocket_event_id_t_WEBSOCKET_EVENT_ANY,
                Some(on_event),
                arg,
            )
        })
        .context("register WebSocket events")?;
        // SAFETY: `handle` is valid and its events are registered.
        esp!(unsafe { esp_websocket_client_start(handle) }).context("start WebSocket")?;
        Ok(client)
    }

    pub fn send_text(&self, text: &str, timeout: Duration) -> anyhow::Result<()> {
        let len = i32::try_from(text.len()).context("message too long")?;
        // SAFETY: `handle` is valid; the client copies `len` bytes from
        // `text` before returning.
        let sent = unsafe {
            esp_websocket_client_send_text(
                self.handle,
                text.as_ptr().cast(),
                len,
                TickType::from(timeout).ticks(),
            )
        };
        if sent < 0 {
            bail!("WebSocket send failed");
        }
        Ok(())
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        // SAFETY: `handle` is valid, and this isn't the client's own task.
        // Close sends a close frame if still connected; destroy stops the
        // task, so `on_event` can't run once the box is freed after this.
        unsafe {
            if esp_websocket_client_is_connected(self.handle) {
                esp_websocket_client_close(self.handle, TickType::from(CLOSE_TIMEOUT).ticks());
            }
            esp_websocket_client_destroy(self.handle);
        }
    }
}

/// Runs on the client's task: copy and pass on, nothing more.
unsafe extern "C" fn on_event(
    arg: *mut c_void,
    _base: esp_event_base_t,
    id: i32,
    data: *mut c_void,
) {
    // SAFETY: `arg` is the `Sender` registered in `connect`, alive until the
    // client is destroyed; `data` is the client's event data for this call.
    let (events, data) = unsafe {
        (
            &*(arg as *const Sender<WsEvent>),
            (data as *const esp_websocket_event_data_t).as_ref(),
        )
    };

    #[allow(non_upper_case_globals)]
    let event = match id {
        esp_websocket_event_id_t_WEBSOCKET_EVENT_CONNECTED => WsEvent::Connected,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_DISCONNECTED
        | esp_websocket_event_id_t_WEBSOCKET_EVENT_CLOSED => WsEvent::Disconnected,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_ERROR => WsEvent::Error,
        esp_websocket_event_id_t_WEBSOCKET_EVENT_DATA => {
            let Some(d) = data else { return };
            // 0 = continuation, 1 = text, 2 = binary; the rest are control
            // frames (ping, pong, close) the client handles itself.
            if d.op_code > 2 || d.data_ptr.is_null() || d.data_len <= 0 {
                return;
            }
            // SAFETY: the client guarantees `data_len` readable bytes at
            // `data_ptr` for the duration of this call.
            let bytes =
                unsafe { std::slice::from_raw_parts(d.data_ptr.cast::<u8>(), d.data_len as usize) };
            WsEvent::Data {
                bytes: bytes.to_vec(),
                frame_len: usize::try_from(d.payload_len).unwrap_or(0),
                offset: usize::try_from(d.payload_offset).unwrap_or(0),
            }
        }
        _ => return,
    };
    // The receiver is gone only while shutting down.
    let _ = events.send(event);
}
