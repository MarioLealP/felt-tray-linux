//! WP-12: procedural face life - idle micro-expressions so she isn't just
//! blinking and lipsyncing between real reactions, plus speech-envelope-
//! driven brow raises so her face accents the actual audio. Pure functions,
//! no overlay/async here - the face-life loop does the orchestration.

/// Injectable source of randomness so tests can be deterministic.
pub trait Rng {
    /// A uniformly chosen integer in `0..n`. Callers in this crate always
    /// pass `n > 0`.
    fn below(&mut self, n: usize) -> usize;
}

// "a flicker of soft_smile, a brow raise (curious), a tiny pout" - WP-12's
// own list. "glance-and-smile" is left out - it needs gaze (WP-14), which
// doesn't exist yet.
/// Poses an idle micro-expression may pick from.
pub const MICRO_EXPRESSION_POOL: [&str; 3] = ["soft_smile", "curious", "pout"];

/// Minimum seconds between two idle micro-expressions.
pub const MICRO_EXPRESSION_MIN_INTERVAL_S: f64 = 5.0;
/// Maximum seconds between two idle micro-expressions.
pub const MICRO_EXPRESSION_MAX_INTERVAL_S: f64 = 20.0;

/// Intensity of the "curious" micro-expression.
pub const CURIOUS_INTENSITY: f64 = 0.3;
/// How long the "curious" micro-expression is held, in seconds.
pub const CURIOUS_HOLD_S: f64 = 0.6;

/// Random, but never an immediate repeat of the last one picked -
/// WP-12: "Weighted, with no immediate repeats."
pub fn pick_micro_expression<R: Rng + ?Sized>(rng: &mut R, last: Option<&str>) -> &'static str {
    let mut choices = MICRO_EXPRESSION_POOL;
    let mut n_choices = 0;
    for pose in MICRO_EXPRESSION_POOL {
        if last.is_none_or(|l| l != pose) {
            choices[n_choices] = pose;
            n_choices += 1;
        }
    }
    let choices: &[&str] = if n_choices == 0 {
        &MICRO_EXPRESSION_POOL
    } else {
        &choices[..n_choices]
    };
    choices[rng.below(choices.len())]
}

// -- Speech envelope accents -----------------------------------------

const ENVELOPE_WINDOW_MS: u32 = 50;
const PEAK_RATIO: f64 = 1.5;

/// Intensity of the brow raise triggered by a speech-envelope peak.
pub const BROW_RAISE_INTENSITY: f64 = 0.25;
/// How long the brow raise is held, in seconds.
pub const BROW_RAISE_HOLD_S: f64 = 0.25;

// Audit 2026-09-24: peaks were "any 50 ms window above 1.5x the WHOLE
// clip's mean", so one loud syllable produced a run of consecutive peaks
// (a flood of accent messages). Now: a trailing ~1 s rolling mean, one
// peak per above-threshold run (its loudest window), and a refractory gap
// a little longer than main.gd's own 450 ms accent refractory.
const ROLLING_MEAN_S: f64 = 1.0;
/// Minimum seconds between two detected peaks.
pub const PEAK_REFRACTORY_S: f64 = 0.5;
const MIN_HISTORY_WINDOWS: usize = 4;
const NOISE_FLOOR_OF_MAX: f64 = 0.05;

/// RMS over `window_ms` windows; a window whose RMS exceeds `peak_ratio`
/// times the mean of the preceding ~1 s (the whole clip's mean until there's
/// enough history) starts a peak. Each above-threshold run yields one peak,
/// at its loudest window, and peaks are at least `refractory_s` apart.
/// Returns peak times in seconds from the start of the audio, for the
/// face-life loop to schedule a brief brow-raise against as the reply
/// actually plays.
///
/// `frames` are audio samples in `channels`-wide interleaved layout
/// (mono is `channels == 1`); multi-channel input is averaged down to mono
/// first, like `frames.mean(axis=1)` in the original. Non-`f64` sample
/// types (e.g. `i16` PCM) are converted as-is. Shorter than one window, or
/// silent audio, gives no peaks. Tuning knobs are
/// [`detect_envelope_peaks_with`].
pub fn detect_envelope_peaks<S: Copy + Into<f64>>(
    frames: &[S],
    channels: usize,
    samplerate: u32,
) -> Vec<f64> {
    detect_envelope_peaks_with(
        frames,
        channels,
        samplerate,
        ENVELOPE_WINDOW_MS,
        PEAK_RATIO,
        PEAK_REFRACTORY_S,
    )
}

/// [`detect_envelope_peaks`] with explicit `window_ms`, `peak_ratio` and
/// `refractory_s` (the original function's keyword arguments).
pub fn detect_envelope_peaks_with<S: Copy + Into<f64>>(
    frames: &[S],
    channels: usize,
    samplerate: u32,
    window_ms: u32,
    peak_ratio: f64,
    refractory_s: f64,
) -> Vec<f64> {
    if frames.is_empty() {
        return Vec::new();
    }
    assert!(channels >= 1, "channels must be at least 1");
    assert!(samplerate >= 1, "samplerate must be at least 1");
    assert!(window_ms >= 1, "window_ms must be at least 1");

    // Average interleaved multi-channel samples down to mono, like
    // `frames.mean(axis=1)`.
    let mono: Vec<f64> = if channels == 1 {
        frames.iter().map(|s| (*s).into()).collect()
    } else {
        let chunks = frames.chunks_exact(channels);
        assert!(
            chunks.remainder().is_empty(),
            "sample count must be a multiple of channels"
        );
        chunks
            .map(|frame| frame.iter().map(|s| (*s).into()).sum::<f64>() / channels as f64)
            .collect()
    };

    // `max(1, int(samplerate * window_ms / 1000))` - truncating, like `int()`.
    let window_size = ((u64::from(samplerate) * u64::from(window_ms)) / 1000).max(1) as usize;
    let n_windows = mono.len() / window_size;
    if n_windows == 0 {
        return Vec::new();
    }

    // RMS of each full window.
    let rms: Vec<f64> = mono[..n_windows * window_size]
        .chunks_exact(window_size)
        .map(|window| {
            let mean_sq = window.iter().map(|x| x * x).sum::<f64>() / window_size as f64;
            mean_sq.sqrt()
        })
        .collect();

    let global_mean = rms.iter().sum::<f64>() / rms.len() as f64;
    if global_mean <= 0.0 {
        return Vec::new();
    }
    let max_rms = rms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let floor = NOISE_FLOOR_OF_MAX * max_rms;
    // `max(1, int(round(_ROLLING_MEAN_S * 1000 / window_ms)))` - Python's
    // `round` on floats is round-half-to-even.
    let history_windows = ROLLING_MEAN_S * 1000.0 / f64::from(window_ms);
    let history = (history_windows.round_ties_even() as usize).max(1);
    let window_s = window_size as f64 / f64::from(samplerate);

    let mut peaks: Vec<f64> = Vec::new();
    let mut run_best: Option<usize> = None;
    for (i, &r) in rms.iter().enumerate() {
        let lo = i.saturating_sub(history);
        let reference = if i - lo >= MIN_HISTORY_WINDOWS {
            rms[lo..i].iter().sum::<f64>() / (i - lo) as f64
        } else {
            global_mean
        };
        let above = r > peak_ratio * reference && r > floor;
        if above {
            if run_best.is_none_or(|best| r > rms[best]) {
                run_best = Some(i);
            }
            continue;
        }
        if let Some(best) = run_best.take() {
            let t = best as f64 * window_s;
            if peaks.last().is_none_or(|p| t - *p >= refractory_s) {
                peaks.push(t);
            }
        }
    }
    if let Some(best) = run_best {
        let t = best as f64 * window_s;
        if peaks.last().is_none_or(|p| t - *p >= refractory_s) {
            peaks.push(t);
        }
    }
    peaks
}
