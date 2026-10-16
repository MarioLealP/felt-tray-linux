//! TempoLock: the rolling stability state machine that decides when a run
//! of tempo estimates is a real, sustained beat.

use std::sync::OnceLock;
use std::time::Instant;

/// WP-16's own "BPM within +-4%".
pub const BPM_TOLERANCE: f64 = 0.04;
/// WP-16's own "for >= 5s".
pub const LOCK_HOLD_S: f64 = 5.0;
/// estimate_tempo_acf's confidence scale (see its docstring).
pub const MIN_CONFIDENCE: f64 = 0.15;

const ACQUIRE_AGREEMENT: f64 = 0.8; // share of the window that must agree to lock
const KEEP_AGREEMENT: f64 = 0.5; // share that must still agree to stay locked

/// `time.monotonic` equivalent for the default clock: seconds since an
/// arbitrary fixed point, monotonic across calls.
pub fn monotonic_seconds() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Clock-injectable (like presence.py/hush.py). `update()` is called
/// once per estimate (server.py: every second, over the trailing 8 s of
/// audio) and judges the last [`LOCK_HOLD_S`] seconds of estimates as a
/// whole:
///
/// - lock when the window spans >= [`LOCK_HOLD_S`] and >= 80 % of its
///   estimates are confident and within +-[`BPM_TOLERANCE`] of their median
///   (the spec's "BPM within +-4 % for >= 5 s", robust to one stray
///   tick);
/// - stay locked while >= 50 % still agree with the locked BPM, so one
///   missed or octave-confused tick doesn't stop the head-bob; music
///   stopping (all None) unlocks within ~3 s.
///
/// A `None` estimate (energy below the floor, too little audio) and a
/// low-confidence one both count as "not agreeing".
pub struct TempoLock<F: FnMut() -> f64> {
    now_fn: F,
    min_confidence: f64,
    /// (timestamp, bpm or None) - one entry per `update` call.
    window: Vec<(f64, Option<f64>)>,
    /// True once the recent window looks like a sustained beat.
    pub locked: bool,
    /// The locked BPM (drift-followed while locked); 0.0 when unlocked.
    pub bpm: f64,
}

impl TempoLock<fn() -> f64> {
    /// Like Python's `TempoLock()`: defaults to a monotonic clock.
    pub fn new() -> Self {
        Self::with_clock(monotonic_seconds)
    }
}

impl Default for TempoLock<fn() -> f64> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: FnMut() -> f64> TempoLock<F> {
    /// Like Python's `TempoLock(now_fn=...)`: the clock is injected so
    /// tests stay deterministic.
    pub fn with_clock(now_fn: F) -> Self {
        Self {
            now_fn,
            min_confidence: MIN_CONFIDENCE,
            window: Vec::new(),
            locked: false,
            bpm: 0.0,
        }
    }

    /// Like Python's `TempoLock(min_confidence=...)`.
    pub fn with_min_confidence(mut self, min_confidence: f64) -> Self {
        self.min_confidence = min_confidence;
        self
    }

    /// Feed one `(bpm, confidence)` estimate (or `None` when the window
    /// had too little audio). `now` defaults to the injected clock.
    /// Returns whether a lock is held after this tick.
    pub fn update(&mut self, estimate: Option<(f64, f64)>, now: Option<f64>) -> bool {
        let now = now.unwrap_or_else(|| (self.now_fn)());
        let bpm = match estimate {
            Some((b, confidence)) if confidence >= self.min_confidence => Some(b),
            _ => None,
        };
        self.window.push((now, bpm));
        let cutoff = now - LOCK_HOLD_S;
        self.window.retain(|&(t, _)| t >= cutoff);
        let span = self.window[self.window.len() - 1].0 - self.window[0].0;
        let values: Vec<Option<f64>> = self.window.iter().map(|&(_, b)| b).collect();

        if self.locked {
            // `b.is_some()` is redundant with agrees() itself already
            // requiring a non-None bpm - spelled out so the iterator
            // narrows to float for the median() call below.
            let agreeing: Vec<f64> = values
                .iter()
                .copied()
                .filter(|&b| agrees(b, self.bpm))
                .flatten()
                .collect();
            if agreeing.len() as f64 >= KEEP_AGREEMENT * values.len() as f64 {
                // Follow slow drift within tolerance (most recent agreeing tick).
                if agrees(bpm, self.bpm) {
                    self.bpm = median(&agreeing);
                }
                return true;
            }
            self.locked = false;
        }

        let confident: Vec<f64> = values.iter().copied().flatten().collect();
        if span < LOCK_HOLD_S || confident.is_empty() {
            self.locked = false;
            return false;
        }
        let reference = median(&confident);
        let agreeing: Vec<f64> = values
            .iter()
            .copied()
            .filter(|&b| agrees(b, reference))
            .flatten()
            .collect();
        if agreeing.len() as f64 >= ACQUIRE_AGREEMENT * values.len() as f64 {
            self.locked = true;
            self.bpm = median(&agreeing);
        }
        self.locked
    }

    /// Drop all history and unlock.
    pub fn reset(&mut self) {
        self.window.clear();
        self.locked = false;
        self.bpm = 0.0;
    }
}

/// True when `bpm` is a confident estimate within `BPM_TOLERANCE` of
/// `reference` (and `reference` is even usable as a baseline).
fn agrees(bpm: Option<f64>, reference: f64) -> bool {
    bpm.is_some_and(|b| reference > 0.0 && (b - reference).abs() / reference <= BPM_TOLERANCE)
}

/// Mean of the two middle values for an even count, like numpy.median.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("tempo estimates must not be NaN"));
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}
