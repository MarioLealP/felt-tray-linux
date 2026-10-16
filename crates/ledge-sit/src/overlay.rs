//! `overlay.OverlayClient`'s `{"seated": ...}` plumbing: a real notice
//! from the wire resolves a registered waiter (what
//! `wait_for_seated` hangs off). The connection handling and the other
//! notice kinds (finished/landed/climbed/on_message) stay in overlay.py;
//! this crate is transport-free, so `read_loop` reads newline-delimited
//! JSON text from any `BufRead`.

use std::cell::Cell;
use std::io::BufRead;
use std::rc::Rc;

/// A pending `wait_for_seated` - asyncio.Future's `result()` surface.
pub struct SeatedWaiter {
    value: Cell<Option<bool>>,
}

impl SeatedWaiter {
    /// `fut.result()` - None while still pending, Some(value) once the
    /// `{"seated": ...}` notice resolved it.
    pub fn result(&self) -> Option<bool> {
        self.value.get()
    }

    fn resolve(&self, seated: bool) {
        self.value.set(Some(seated));
    }
}

/// `overlay.OverlayClient` reduced to the seated-notice plumbing.
pub struct OverlayClient {
    /// `OverlayClient._seated_waiters`.
    pub seated_waiters: Vec<Rc<SeatedWaiter>>,
}

impl Default for OverlayClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayClient {
    pub fn new() -> Self {
        Self {
            seated_waiters: Vec::new(),
        }
    }

    /// Arms a `wait_for_seated` future (appends to `_seated_waiters`).
    pub fn add_seated_waiter(&mut self) -> Rc<SeatedWaiter> {
        let waiter = Rc::new(SeatedWaiter {
            value: Cell::new(None),
        });
        self.seated_waiters.push(Rc::clone(&waiter));
        waiter
    }

    /// `OverlayClient._read_loop`'s seated branch: an `isinstance
    /// (msg.get("seated"), bool)` notice resolves every armed waiter
    /// (Python drains the list). Unparseable lines are skipped.
    pub fn read_loop<R: BufRead>(&mut self, reader: R) {
        for line in reader.lines() {
            let Ok(text) = line else { break };
            let Some(seated) = parse_bool_field(&text, "seated") else {
                continue;
            };
            let waiters = std::mem::take(&mut self.seated_waiters);
            for waiter in &waiters {
                waiter.resolve(seated);
            }
        }
    }
}

/// The `{"seated": true|false}` subset of JSON this plumbing needs:
/// finds `"key"` as a member name and reads the literal bool after it.
/// A non-bool value returns None (Python's `isinstance(..., bool)` guard).
fn parse_bool_field(line: &str, key: &str) -> Option<bool> {
    let pattern = format!("\"{key}\"");
    let mut rest = line;
    while let Some(idx) = rest.find(&pattern) {
        rest = &rest[idx + pattern.len()..];
        let after_key = rest.trim_start();
        let Some(value) = after_key.strip_prefix(':') else {
            // The key text appeared somewhere other than before a ':'.
            rest = after_key;
            continue;
        };
        let value = value.trim_start();
        if value.starts_with("true") {
            return Some(true);
        }
        if value.starts_with("false") {
            return Some(false);
        }
        return None;
    }
    None
}
