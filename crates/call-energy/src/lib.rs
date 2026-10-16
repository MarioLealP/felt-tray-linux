//! WP-17: call liveliness + a "friend joined" heuristic, built entirely
//! on top of the WP-09 call_monitor's existing peak/talking stream
//! (call_monitor.CallMonitor.last_peak/.talking) - no new audio capture
//! here, only decision logic on values that already exist. Amplitude only,
//! never content, same privacy boundary WP-09 already established.
//!
//! Port of `felt_animation.signals.call_energy`. The clock is injectable
//! (Python's `now_fn=time.monotonic` default becomes an explicit argument)
//! so tests stay deterministic.

/// "sustained high energy/laughter-ish bursts" - deliberately a higher bar
/// than call_monitor's own "is anyone talking at all" threshold
/// (FELT_CALL_PEAK_THRESHOLD, ~0.02 default, itself still uncalibrated
/// against a real call per WP-09's own notes). This is equally
/// uncalibrated - a starting guess, not a measured value.
pub const LIVELY_PEAK_THRESHOLD: f64 = 0.08;
/// How long the smoothed peak must stay above the threshold before "lively".
pub const LIVELY_SUSTAIN_S: f64 = 5.0;
/// How long the smoothed peak must stay below the threshold before "quiet".
pub const QUIET_HANG_S: f64 = 8.0;
/// Exponential moving average weight per sample.
pub const ENERGY_SMOOTHING: f64 = 0.3;

/// The WP's own number: "a new Discord audio burst after > 2 min of silence".
pub const JOIN_GUESS_SILENCE_S: f64 = 120.0;

/// Event returned by [`CallEnergyTracker::update`] when sustained high energy starts.
pub const LIVELY_EVENT: &str = "lively";
/// Event returned by [`CallEnergyTracker::update`] when energy dropped and hung low.
pub const QUIET_EVENT: &str = "quiet";

/// Clock-injectable (same style as call_monitor.TalkingDetector).
/// Feed it real peak samples from the same CallMonitor already polling
/// for WP-09's ducking; returns "lively"/"quiet"/None per update,
/// edge-triggered (fires once per transition, not every sample the
/// level stays high/low).
pub struct CallEnergyTracker<F> {
    lively_threshold: f64,
    sustain_s: f64,
    quiet_hang_s: f64,
    smoothing: f64,
    now_fn: F,
    smoothed: f64,
    /// Current liveliness state (Python's public `self.lively`).
    pub lively: bool,
    above_since: Option<f64>,
    below_since: Option<f64>,
}

impl<F: FnMut() -> f64> CallEnergyTracker<F> {
    /// Creates a tracker. Parameter defaults in Python were
    /// [`LIVELY_PEAK_THRESHOLD`], [`LIVELY_SUSTAIN_S`], [`QUIET_HANG_S`],
    /// [`ENERGY_SMOOTHING`] and `now_fn=time.monotonic`; the clock is
    /// always injected here instead.
    pub fn new(
        lively_threshold: f64,
        sustain_s: f64,
        quiet_hang_s: f64,
        smoothing: f64,
        now_fn: F,
    ) -> Self {
        Self {
            lively_threshold,
            sustain_s,
            quiet_hang_s,
            smoothing,
            now_fn,
            smoothed: 0.0,
            lively: false,
            above_since: None,
            below_since: None,
        }
    }

    /// Feeds one peak sample from the call_monitor stream. Returns
    /// `Some("lively")` / `Some("quiet")` on a transition and `None`
    /// otherwise.
    pub fn update(&mut self, peak: f64) -> Option<&'static str> {
        self.smoothed += self.smoothing * (peak - self.smoothed);
        let now = (self.now_fn)();

        if !self.lively {
            if self.smoothed < self.lively_threshold {
                self.above_since = None;
                return None;
            }
            // First sample above threshold starts the sustain window.
            let above_since = *self.above_since.get_or_insert(now);
            if now - above_since < self.sustain_s {
                return None;
            }
            self.lively = true;
            self.above_since = None;
            return Some(LIVELY_EVENT);
        }

        if self.smoothed >= self.lively_threshold {
            self.below_since = None;
            return None;
        }
        // First sample below threshold starts the quiet-hang window.
        let below_since = *self.below_since.get_or_insert(now);
        if now - below_since < self.quiet_hang_s {
            return None;
        }
        self.lively = false;
        self.below_since = None;
        Some(QUIET_EVENT)
    }
}

/// A new talking burst after a long silence, on the same `talking`
/// stream call_monitor.TalkingDetector already produces. A heuristic,
/// not a real "someone joined" signal - a returning silent person
/// un-muting after a long quiet stretch looks identical. That limit is
/// intentional, not hidden - see the WP's own "note its limits".
pub struct CallJoinGuessTracker<F> {
    silence_threshold_s: f64,
    now_fn: F,
    was_talking: bool,
    silence_started_at: Option<f64>,
}

impl<F: FnMut() -> f64> CallJoinGuessTracker<F> {
    /// Creates a tracker. The Python default was
    /// `silence_threshold_s=JOIN_GUESS_SILENCE_S, now_fn=time.monotonic`;
    /// the clock is always injected here instead.
    pub fn new(silence_threshold_s: f64, now_fn: F) -> Self {
        Self {
            silence_threshold_s,
            now_fn,
            was_talking: false,
            silence_started_at: None,
        }
    }

    /// Feeds one `talking` sample. Returns `true` exactly once when a new
    /// talking burst starts after at least `silence_threshold_s` of silence.
    pub fn update(&mut self, talking: bool) -> bool {
        let now = (self.now_fn)();
        if !talking {
            if self.was_talking {
                self.silence_started_at = Some(now);
            }
            self.was_talking = false;
            return false;
        }

        let mut fires = false;
        if !self.was_talking
            && let Some(silence_started_at) = self.silence_started_at
        {
            fires = (now - silence_started_at) >= self.silence_threshold_s;
        }
        self.was_talking = true;
        fires
    }
}
