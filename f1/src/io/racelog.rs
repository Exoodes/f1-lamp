//! The race log's lines and the drivers' short names, shared by the render
//! loop (which logs), the live feed (which knows the names) and the web
//! server (which serves the lines). Like `weblog`, kept in memory only.

use std::{collections::HashMap, sync::Mutex};

use f1_core::racelog::{LogItem, RaceLog, RaceLogPage};

use crate::io::clock;

static LOG: Mutex<RaceLog> = Mutex::new(RaceLog::new());
static NAMES: Mutex<Option<HashMap<u8, String>>> = Mutex::new(None);

/// Remembers car `number`'s short name, e.g. `VER`.
pub fn set_name(number: u8, name: String) {
    NAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(number, name);
}

/// Car `number`'s short name, if known.
pub fn name(number: u8) -> Option<String> {
    NAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|n| n.get(&number).cloned())
}

/// Logs `items`, stamped with the time now: when the lamp shows them, in
/// step with the TV.
pub fn record(items: Vec<LogItem>) {
    if items.is_empty() {
        return;
    }
    let at = clock::second_of_day();
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    for item in items {
        log.push(item, at);
    }
}

/// The lines from number `after` on, written with the names known now.
pub fn page(after: Option<u64>) -> RaceLogPage {
    let names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let name = |number: u8| {
        names
            .as_ref()
            .and_then(|n| n.get(&number).cloned())
            .unwrap_or_else(|| format!("#{number}"))
    };
    LOG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .since(after, &name)
}
