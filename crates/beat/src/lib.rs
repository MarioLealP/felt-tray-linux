//! WP-16: tempo tracking from loopback audio, pure DSP - no device I/O
//! here (see beat_capture.py for that), so this is directly unit-testable
//! against synthetic onset streams.
//!
//! **Not aubio.** The WP's own text names aubio's `aubio.tempo` as the
//! tracker to use; it has no prebuilt wheel for the original machine's
//! Python (3.12, Windows) and building from source needs a C++ toolchain
//! (MSVC) that isn't installed - not worth pulling in Visual C++ Build
//! Tools for a nice-to-have head-bob feature. The Python original is a
//! from-scratch, numpy-only replacement, ported here dependency-free:
//! RMS-based onset picking (same "energy exceeds a ratio of its own
//! recent local mean" idea as face_life.py's speech-envelope-peak
//! detector, WP-12) -> consecutive inter-onset intervals, clustered with
//! a tolerance to find the dominant beat period (correcting for the fact
//! that raw IOIs already tend to agree once octave-doubled/halved
//! outliers are outvoted, rather than a separate octave-correction pass)
//! -> a rolling stability check ([`TempoLock`]) deciding when that's a
//! real, sustained beat rather than a one-off coincidence. Cruder than
//! aubio's own onset-detection-function-plus-comb-filter approach -
//! flagged here so a future aubio swap-in (once/if a compiler toolchain
//! exists) is an easy drop-in, not an archaeology project.

#![forbid(unsafe_code)]

mod fft;
mod lock;
mod tempo;

pub use lock::{BPM_TOLERANCE, LOCK_HOLD_S, MIN_CONFIDENCE, TempoLock, monotonic_seconds};
pub use tempo::{
    ACF_MAX_BPM, ACF_MIN_BPM, beat_phase, estimate_tempo, estimate_tempo_acf,
    estimate_tempo_from_audio, inter_onset_intervals, onset_envelope,
};

// -- onset detection ---------------------------------------------------

const ONSET_WINDOW_MS: u32 = 20;
const ONSET_PEAK_RATIO: f64 = 1.6; // a window's RMS must exceed this x its own rolling local mean
const ONSET_LOCAL_MEAN_WINDOWS: usize = 20; // ~0.4s of trailing windows for the local mean
const ONSET_MIN_GAP_S: f64 = 0.12; // refractory period - a single hit shouldn't fire twice (~500 BPM cap)

/// Mono `frames` -> per-window RMS (numpy's default 20 ms window).
/// Same shape of computation as face_life.detect_envelope_peaks's
/// windowing, reused rather than duplicated in spirit (different module,
/// since this one has no dependency on that one and is meant to stand
/// alone as beat.py).
///
/// The Python also accepts a `(samples, channels)` array and averages
/// channels first - downmix to mono before calling this.
pub fn rms_windows(frames: &[f64], samplerate: u32) -> Vec<f64> {
    rms_windows_with(frames, samplerate, ONSET_WINDOW_MS)
}

/// [`rms_windows`] with an explicit window length in milliseconds.
pub fn rms_windows_with(frames: &[f64], samplerate: u32, window_ms: u32) -> Vec<f64> {
    if frames.is_empty() {
        return Vec::new();
    }
    let window_size = ((samplerate as f64 * window_ms as f64) / 1000.0) as usize;
    let window_size = window_size.max(1);
    let n_windows = frames.len() / window_size;
    if n_windows == 0 {
        return Vec::new();
    }
    // Only whole windows count; the tail of the buffer is dropped.
    frames[..n_windows * window_size]
        .chunks(window_size)
        .map(|w| (w.iter().map(|&s| s * s).sum::<f64>() / w.len() as f64).sqrt())
        .collect()
}

/// Onset times (seconds from the start of `rms`'s buffer) with the
/// default detector parameters - see [`pick_onsets_with`].
pub fn pick_onsets(rms: &[f64], window_s: f64) -> Vec<f64> {
    pick_onsets_with(
        rms,
        window_s,
        ONSET_PEAK_RATIO,
        ONSET_LOCAL_MEAN_WINDOWS,
        ONSET_MIN_GAP_S,
    )
}

/// Onset times (seconds from the start of `rms`'s buffer) - a window
/// whose RMS exceeds `peak_ratio` times the mean of the preceding
/// `local_mean_windows` windows counts as an onset, with a refractory
/// gap so a single transient's decay tail can't register as several.
pub fn pick_onsets_with(
    rms: &[f64],
    window_s: f64,
    peak_ratio: f64,
    local_mean_windows: usize,
    min_gap_s: f64,
) -> Vec<f64> {
    if rms.is_empty() {
        return Vec::new();
    }
    let mut onsets = Vec::new();
    let mut last_onset_t = -min_gap_s;
    for (i, &r) in rms.iter().enumerate() {
        let lo = i.saturating_sub(local_mean_windows);
        let local_mean = if i > lo {
            rms[lo..i].iter().sum::<f64>() / (i - lo) as f64
        } else {
            0.0
        };
        // A near-silent local mean (true digital silence between hits, or
        // a very quiet run) must NOT suppress detection entirely - a tiny
        // floor keeps the ratio test meaningful instead of comparing
        // against a hard zero that no `local_mean <= 0: continue` guard
        // could ever clear.
        let effective_mean = local_mean.max(1e-6);
        if r > peak_ratio * effective_mean {
            let t = i as f64 * window_s;
            if t - last_onset_t >= min_gap_s {
                onsets.push(t);
                last_onset_t = t;
            }
        }
    }
    onsets
}
