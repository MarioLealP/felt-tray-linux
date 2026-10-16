//! Tempo estimation: inter-onset-interval clustering over onset times, and
//! the spectral-flux + autocorrelation estimator added after the 2026-09-24
//! audit. Pure DSP - no device I/O lives here.

use crate::fft::fft;

// -- tempo estimation from onsets ---------------------------------------

pub(crate) const MIN_INTERVAL_S: f64 = 60.0 / 200.0; // 200 BPM upper bound
pub(crate) const MAX_INTERVAL_S: f64 = 60.0 / 50.0; // 50 BPM lower bound
pub(crate) const CLUSTER_TOLERANCE: f64 = 0.06; // 6% relative tolerance when grouping IOIs
pub(crate) const MIN_IOIS_FOR_ESTIMATE: usize = 4;

/// Consecutive onset gaps, keeping only strictly increasing pairs
/// (onset_times[1:] is deliberately shorter than onset_times by one -
/// pairs consecutive onsets, so strict matching would be wrong here).
pub fn inter_onset_intervals(onset_times: &[f64]) -> Vec<f64> {
    onset_times
        .windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| w[1] - w[0])
        .collect()
}

/// (bpm, confidence 0..1) from a batch of onset timestamps (ascending
/// seconds), or None with too little to go on. Confidence is the
/// fraction of in-range consecutive IOIs that agree with the winning
/// cluster (within [`CLUSTER_TOLERANCE`]) - a practical "how much of the
/// beat grid agrees" measure, not a real statistical confidence.
pub fn estimate_tempo(onset_times: &[f64]) -> Option<(f64, f64)> {
    let iois: Vec<f64> = inter_onset_intervals(onset_times)
        .into_iter()
        .filter(|&d| (MIN_INTERVAL_S..=MAX_INTERVAL_S).contains(&d))
        .collect();
    if iois.len() < MIN_IOIS_FOR_ESTIMATE {
        return None;
    }

    // First candidate with the most votes wins ties, like numpy's argmax.
    let mut best_interval: Option<f64> = None;
    let mut best_count = 0usize;
    for &candidate in &iois {
        let count = iois
            .iter()
            .filter(|&&d| (d - candidate).abs() <= CLUSTER_TOLERANCE * candidate)
            .count();
        if count > best_count {
            best_count = count;
            best_interval = Some(candidate);
        }
    }
    let best_interval = best_interval?;

    let agreeing: Vec<f64> = iois
        .iter()
        .copied()
        .filter(|&d| (d - best_interval).abs() <= CLUSTER_TOLERANCE * best_interval)
        .collect();
    // `agreeing` is never empty - best_interval agrees with itself.
    let interval = agreeing.iter().sum::<f64>() / agreeing.len() as f64;
    let bpm = 60.0 / interval;
    let confidence = best_count as f64 / iois.len() as f64;
    Some((bpm, confidence))
}

/// 0..1 position within the current beat, `bpm` beats/min, anchored
/// at `reference_time` (a real onset time - the phase doesn't have to
/// originate AT that instant, only share its grid).
pub fn beat_phase(now: f64, reference_time: f64, bpm: f64) -> f64 {
    if bpm <= 0.0 {
        return 0.0;
    }
    let interval = 60.0 / bpm;
    let elapsed = now - reference_time;
    // Python's float `%` is floored; fold negatives back into
    // [0, interval) so a `now` before `reference_time` is still 0..1.
    let m = elapsed % interval;
    let m = if m < 0.0 { m + interval } else { m };
    m / interval
}

// -- spectral-flux + autocorrelation tempo (audit 2026-09-24) -------------
//
// Why the RMS-onset/IOI path above never locked on real music (measured on
// 7 real tracks from Mario's library, 8 s windows every 1 s, the exact live
// loop): the *estimates* were often right (105, 110, 126, 132 BPM ...), but
// (1) a single-band RMS onset picker only finds ~2 onsets/s in dense,
// compressed mixes (2 onsets in 8 s on one track), so the IOI "confidence"
// (share of IOIs agreeing) swung between 0.3 and 0.8 tick to tick, and
// (2) TempoLock reset the whole run on ANY single tick below 0.5 confidence
// or off by >4 % - so it needed ~6 perfect ticks in a row, which real music
// almost never gave (0-14 of 62 ticks locked per track). The fix is both:
// an onset-strength envelope (log-magnitude spectral flux) + autocorrelation
// with a tempo prior, which is far steadier window to window, and a lock
// that judges a window of estimates by majority instead of resetting on one
// outlier.

const FLUX_FFT: usize = 1024;
const FLUX_HOP: usize = 512;
const FLUX_MAX_HZ: f64 = 8000.0;
const FLUX_TREND_S: f64 = 0.5;

/// Lower BPM bound of the autocorrelation search.
pub const ACF_MIN_BPM: f64 = 60.0;
/// Upper BPM bound of the autocorrelation search.
pub const ACF_MAX_BPM: f64 = 200.0;

const PRIOR_CENTER_BPM: f64 = 120.0;
const PRIOR_OCTAVE_WIDTH: f64 = 1.0;

/// Mono `frames` -> (onset-strength envelope, envelope frame rate).
/// Half-wave-rectified log-magnitude spectral flux (bins below 8 kHz),
/// minus its own ~0.5 s moving average. Unlike RMS it sees a hi-hat or a
/// snare on top of a sustained, loud mix.
pub fn onset_envelope(frames: &[f64], samplerate: u32) -> (Vec<f64>, f64) {
    let frame_rate = samplerate as f64 / FLUX_HOP as f64;
    if frames.len() < FLUX_FFT * 2 {
        return (Vec::new(), frame_rate);
    }
    let n_frames = 1 + (frames.len() - FLUX_FFT) / FLUX_HOP;

    // numpy's symmetric np.hanning(FLUX_FFT).
    let hann: Vec<f64> = (0..FLUX_FFT)
        .map(|j| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * j as f64 / (FLUX_FFT - 1) as f64).cos())
        .collect();
    // Only bins below FLUX_MAX_HZ count (numpy: freqs < 8000).
    let keep: Vec<usize> = (0..FLUX_FFT / 2 + 1)
        .filter(|&k| k as f64 * samplerate as f64 / (FLUX_FFT as f64) < FLUX_MAX_HZ)
        .collect();

    let mut log_mag = vec![vec![0.0; keep.len()]; n_frames];
    let mut re = vec![0.0; FLUX_FFT];
    let mut im = vec![0.0; FLUX_FFT];
    for (i, row) in log_mag.iter_mut().enumerate() {
        for j in 0..FLUX_FFT {
            re[j] = frames[i * FLUX_HOP + j] * hann[j];
            im[j] = 0.0;
        }
        fft(&mut re, &mut im);
        for (out, &k) in row.iter_mut().zip(&keep) {
            let magnitude = re[k].hypot(im[k]);
            *out = (100.0 * magnitude).ln_1p(); // np.log1p(100 * magnitude)
        }
    }

    // Half-wave-rectified spectral flux along the frame axis.
    let mut flux = vec![0.0; n_frames - 1];
    for (i, f) in flux.iter_mut().enumerate() {
        *f = log_mag[i]
            .iter()
            .zip(&log_mag[i + 1])
            .map(|(&prev, &next)| (next - prev).max(0.0))
            .sum();
    }

    // np.convolve(flux, ones(k)/k, mode="same") with odd k is a zero-padded
    // mean of k samples (divided by k even at the edges) centered on each
    // point - so compute it directly. `| 1` is numpy-side's "make odd".
    let k = std::cmp::max(1, (FLUX_TREND_S * frame_rate).round_ties_even() as i64) | 1;
    let h = ((k - 1) / 2) as usize;
    let mut envelope = vec![0.0; flux.len()];
    for (p, e) in envelope.iter_mut().enumerate() {
        let lo = p.saturating_sub(h);
        let hi = (p + h + 1).min(flux.len());
        let trend = flux[lo..hi].iter().sum::<f64>() / k as f64;
        *e = (flux[p] - trend).max(0.0);
    }
    (envelope, frame_rate)
}

/// (bpm, confidence 0..1) from an onset envelope, or None with < 3 s of
/// it. Autocorrelation, weighted by a log-Gaussian prior around 120 BPM
/// (one octave wide) to settle octave ambiguity, with harmonic support
/// from the double and half lag. Confidence is the normalized
/// autocorrelation at the chosen lag: ~0.2-0.7 for music with a beat,
/// ~0.05-0.12 for speech and noise.
pub fn estimate_tempo_acf(
    envelope: &[f64],
    frame_rate: f64,
    min_bpm: f64,
    max_bpm: f64,
) -> Option<(f64, f64)> {
    let n = envelope.len();
    if (n as f64) < frame_rate * 3.0 {
        return None;
    }
    let mean = envelope.iter().sum::<f64>() / n as f64;
    let e: Vec<f64> = envelope.iter().map(|&x| x - mean).collect();
    // Linear autocorrelation. The Python original reaches this through a
    // zero-padded power-of-two FFT; the direct sum is the same numbers.
    let mut acf = vec![0.0; n];
    for (lag, a) in acf.iter_mut().enumerate() {
        *a = (0..n - lag).map(|i| e[i] * e[i + lag]).sum();
    }
    if acf[0] <= 0.0 {
        return None;
    }
    let acf0 = acf[0];
    for (lag, a) in acf.iter_mut().enumerate() {
        // numpy: (acf / acf[0]) * e.size / np.maximum(e.size - lags, 1)
        *a = (*a / acf0) * n as f64 / (n - lag).max(1) as f64;
    }

    let lo = (60.0 * frame_rate / max_bpm).floor().max(1.0) as i64;
    let hi = (60.0 * frame_rate / min_bpm).ceil() as i64;
    let hi = hi.min(n as i64 / 2 - 1);
    if hi <= lo {
        return None;
    }

    let mut lag = lo;
    let mut best_score = f64::NEG_INFINITY;
    for candidate in lo..=hi {
        let bpm = 60.0 * frame_rate / candidate as f64;
        let z = (bpm / PRIOR_CENTER_BPM).log2() / PRIOR_OCTAVE_WIDTH;
        let prior = (-0.5 * z * z).exp();
        let score = prior
            * (acf[candidate as usize]
                + 0.5 * acf[(2 * candidate).min(n as i64 - 1) as usize]
                + 0.25 * acf[(candidate / 2).max(1) as usize]);
        // Strict > keeps the first argmax on ties, like numpy.
        if score > best_score {
            best_score = score;
            lag = candidate;
        }
    }

    let mut offset = 0.0;
    if (1..n as i64 - 1).contains(&lag) {
        let (a, b, c) = (
            acf[(lag - 1) as usize],
            acf[lag as usize],
            acf[(lag + 1) as usize],
        );
        let denom = a - 2.0 * b + c;
        if denom != 0.0 {
            offset = (0.5 * (a - c) / denom).clamp(-0.5, 0.5);
        }
    }
    let confidence = acf[lag as usize].clamp(0.0, 1.0);
    Some((60.0 * frame_rate / (lag as f64 + offset), confidence))
}

/// Convenience wrapper: [`estimate_tempo_acf`] over a raw mono frame
/// buffer with the default [`ACF_MIN_BPM`]/[`ACF_MAX_BPM`] bounds.
pub fn estimate_tempo_from_audio(frames: &[f64], samplerate: u32) -> Option<(f64, f64)> {
    let (envelope, frame_rate) = onset_envelope(frames, samplerate);
    estimate_tempo_acf(&envelope, frame_rate, ACF_MIN_BPM, ACF_MAX_BPM)
}
