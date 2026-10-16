//! WP-17: rage-clicking detection.
//!
//! Audit 2026-09-24: the counter used to add key presses to clicks, so
//! ordinary fast typing (8+ keys/s is normal) fired "mash". It now counts
//! mouse clicks only - there is no keyboard hook at all any more, which is
//! also the stronger privacy position.
//!
//! Privacy boundary (the WP's own Accept criterion: "No keystroke content
//! anywhere - review the hook code with Mario"): EventCounter's click
//! callback below takes a `button` argument (pynput's listener API requires
//! the callback signature to accept it) but does nothing with it beyond
//! incrementing an integer counter under a lock - never logged, stored,
//! returned, or passed anywhere else. There is no code path in this crate
//! that can ever surface which key was pressed, what was typed, or where a
//! click landed. **This crate should get an explicit read-through with Mario
//! before the keyboard hook runs live**, per the WP's own acceptance check,
//! not just an automated test.
//!
//! MashDetector (the sustained-rate decision logic) is pure and fully
//! tested against synthetic per-second counts, independent of pynput.
//!
//! Rust-port note: the Python wires a global pynput `mouse.Listener` in
//! [`EventCounter::start`]; that OS hook is behind the [`GlobalMouseListener`]
//! trait, which ships without an implementation.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// The WP's own numbers: rage-click rate threshold in events/s.
pub const MASH_RATE_THRESHOLD: f64 = 8.0;
/// The WP's own numbers: the rate must stay at/above threshold this long.
pub const MASH_SUSTAIN_S: f64 = 3.0;
/// Not in the WP's text - added so a long mashing session produces
/// occasional reactions, not one every single second it stays above
/// threshold.
pub const MASH_COOLDOWN_S: f64 = 5.0;

/// Seconds between events-per-second readings fed to [`MashDetector::update`].
pub const SAMPLE_INTERVAL_S: f64 = 1.0;

/// Clock-injectable. Feed it one events-per-second reading per
/// [`SAMPLE_INTERVAL_S`]; returns true the instant a "mash" event should
/// fire (rate at/above threshold, sustained for `sustain_s`), at most
/// once per `cooldown_s`.
pub struct MashDetector<F> {
    rate_threshold: f64,
    sustain_s: f64,
    cooldown_s: f64,
    now_fn: F,
    high_since: Option<f64>,
    last_fired_at: Option<f64>,
}

impl<F: FnMut() -> f64> MashDetector<F> {
    /// Mirrors the Python constructor's positional parameters
    /// (`rate_threshold`, `sustain_s`, `cooldown_s`, `now_fn`).
    pub fn new(rate_threshold: f64, sustain_s: f64, cooldown_s: f64, now_fn: F) -> Self {
        Self {
            rate_threshold,
            sustain_s,
            cooldown_s,
            now_fn,
            high_since: None,
            last_fired_at: None,
        }
    }

    pub fn update(&mut self, events_per_second: f64) -> bool {
        let now = (self.now_fn)();
        if events_per_second < self.rate_threshold {
            self.high_since = None;
            return false;
        }
        // First reading at/above threshold anchors the sustain window.
        let high_since = *self.high_since.get_or_insert(now);
        if now - high_since < self.sustain_s {
            return false;
        }
        if self
            .last_fired_at
            .is_some_and(|last| now - last < self.cooldown_s)
        {
            return false;
        }
        self.last_fired_at = Some(now);
        true
    }
}

impl MashDetector<fn() -> f64> {
    /// All of the Python constructor's defaults: the WP's threshold/sustain,
    /// the reaction cooldown, and a monotonic clock (`time.monotonic`).
    pub fn with_defaults() -> Self {
        Self::new(
            MASH_RATE_THRESHOLD,
            MASH_SUSTAIN_S,
            MASH_COOLDOWN_S,
            monotonic_secs,
        )
    }
}

/// A clock reading seconds from an arbitrary monotonic origin
/// (`time.monotonic` in the Python). Injectable alternative: pass any
/// `FnMut() -> f64` to [`MashDetector::new`].
pub fn monotonic_secs() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Sink for raw mouse-click events (pynput's `on_click` callback shape).
/// Implementations must not store, log, or return `x`/`y`/`button` - see
/// this crate's privacy boundary.
pub trait ClickSink: Send {
    fn on_click(&mut self, x: i32, y: i32, button: &str, pressed: bool);
}

/// The OS-level global mouse hook (Python: `pynput.mouse.Listener`).
/// No implementation ships with this crate - building and driving the
/// hook is platform-specific. [`start`](GlobalMouseListener::start) must
/// deliver every mouse event to the sink (possibly from another thread)
/// until [`stop`](GlobalMouseListener::stop) is called.
pub trait GlobalMouseListener {
    fn start(&mut self, sink: Box<dyn ClickSink>);
    fn stop(&mut self);
}

/// Counts mouse clicks via pynput's global mouse listener -
/// see this crate's docstring for the privacy boundary. `take_count()`
/// reads and resets the counter; call it once per [`SAMPLE_INTERVAL_S`] to
/// get an events-per-second rate for [`MashDetector::update`].
pub struct EventCounter {
    count: Arc<Mutex<u64>>,
    mouse_listener: Option<Box<dyn GlobalMouseListener>>,
}

impl EventCounter {
    pub fn new() -> Self {
        Self {
            count: Arc::new(Mutex::new(0)),
            mouse_listener: None,
        }
    }

    /// pynput's required callback signature - only `pressed` is ever
    /// inspected (to count the press, not the release, so a click isn't
    /// double-counted); x/y/button are unused and nothing here stores them.
    /// (Python: `_on_click`.)
    pub fn on_click(&self, _x: i32, _y: i32, _button: &str, pressed: bool) {
        if pressed {
            record_press(&self.count);
        }
    }

    /// Start counting clicks from `listener`, a platform global-mouse hook.
    /// The Python constructs `pynput.mouse.Listener(on_click=self._on_click)`
    /// here; that OS wiring is out of scope for this crate, so the listener
    /// is injected instead.
    pub fn start(&mut self, mut listener: Box<dyn GlobalMouseListener>) {
        listener.start(Box::new(CountingSink {
            count: Arc::clone(&self.count),
        }));
        self.mouse_listener = Some(listener);
    }

    pub fn stop(&mut self) {
        // The Python also stops a `_key_listener` field here; nothing could
        // ever set it (there is no keyboard hook), so this crate does not
        // carry the vestigial field at all.
        if let Some(listener) = self.mouse_listener.as_mut() {
            listener.stop();
        }
    }

    pub fn take_count(&self) -> u64 {
        let mut count = lock(&self.count);
        let taken = *count;
        *count = 0;
        taken
    }
}

impl Default for EventCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EventCounter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every stored field by name, mirroring `vars(counter)` in the
        // Python privacy-audit tests. The mouse listener is shown only as a
        // handle marker (its internals are platform-owned).
        let count = lock(&self.count);
        f.debug_struct("EventCounter")
            .field("count", &*count)
            .field("mouse_listener", &self.mouse_listener.is_some())
            .finish()
    }
}

/// The sink [`EventCounter::start`] hands to the platform listener: same
/// counting behaviour as [`EventCounter::on_click`], sharing the counter.
struct CountingSink {
    count: Arc<Mutex<u64>>,
}

impl ClickSink for CountingSink {
    fn on_click(&mut self, _x: i32, _y: i32, _button: &str, pressed: bool) {
        if pressed {
            record_press(&self.count);
        }
    }
}

fn record_press(count: &Mutex<u64>) {
    *lock(count) += 1;
}

fn lock(count: &Mutex<u64>) -> MutexGuard<'_, u64> {
    count
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
