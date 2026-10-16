//! Port of ref/felt-animation/tests/test_beat.py (WP-16): onset picking +
//! tempo estimation over synthetic click tracks at known BPM, and the
//! TempoLock stability state machine over synthetic estimate streams -
//! no real audio device involved.

use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use felt_beat::{
    BPM_TOLERANCE, LOCK_HOLD_S, TempoLock, beat_phase, estimate_tempo, estimate_tempo_from_audio,
    inter_onset_intervals, pick_onsets, rms_windows, rms_windows_with,
};

/// Deterministic xorshift64* stand-in for `np.random.default_rng` (the
/// Python test fixtures only need reproducible uniform/normal noise; the
/// assertions are property-based, not bit-exact against numpy's PCG64).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Any nonzero state works; force odd so seed 0 is usable.
        Self(seed.wrapping_mul(2).wrapping_add(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1) with 53 random bits.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// `rng.uniform(-jitter_s, jitter_s)` for the click-track jitter.
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// `rng.standard_normal()` via Box-Muller.
    fn standard_normal(&mut self) -> f64 {
        let u1 = self.unit().max(f64::MIN_POSITIVE); // avoid log(0)
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

/// A train of short loud pulses at `bpm`, silence between - a synthetic
/// "song" with an unambiguous, known tempo.
fn click_track(bpm: f64, duration_s: f64, samplerate: u32, jitter_s: f64) -> Vec<f64> {
    let interval = 60.0 / bpm;
    let mut frames = vec![0.0; (duration_s * samplerate as f64) as usize];
    let mut rng = Rng::new(0);
    let mut t = 0.0;
    let click_len = (0.01 * samplerate as f64) as usize;
    while t < duration_s {
        let jitter = if jitter_s != 0.0 {
            rng.uniform(-jitter_s, jitter_s)
        } else {
            0.0
        };
        let start = ((t + jitter).max(0.0) * samplerate as f64) as usize;
        let end = (start + click_len).min(frames.len());
        if start < frames.len() {
            for f in &mut frames[start..end] {
                *f = 1.0;
            }
        }
        t += interval;
    }
    frames
}

/// Add one hit of `sig` at `at` seconds with an exponential decay envelope
/// (numpy's `np.exp(-np.linspace(0, 6, len))`).
fn hit(x: &mut [f64], sr: u32, at: f64, length: f64, sig: &[f64], gain: f64) {
    let n = x.len();
    let i = (at * sr as f64) as usize;
    let j = (i + (length * sr as f64) as usize).min(n);
    if i < n {
        let m = j - i;
        for q in 0..m {
            let lin = if m > 1 {
                6.0 * q as f64 / (m - 1) as f64
            } else {
                0.0
            };
            x[i + q] += gain * (-lin).exp() * sig[q];
        }
    }
}

/// Kick on every beat, snare on 2 and 4, hats on the eighths, under a
/// loud sustained chord and noise - dense and compressed, the case the old
/// RMS onset picker couldn't handle.
fn drum_pattern(bpm: f64, duration_s: f64, sr: u32, seed: u64) -> Vec<f64> {
    let mut rng = Rng::new(seed);
    let n = (duration_s * sr as f64) as usize;
    let mut x = vec![0.0; n];
    for (i, xi) in x.iter_mut().enumerate() {
        let t = i as f64 / sr as f64;
        *xi = 0.3
            * ((2.0 * PI * 220.0 * t).sin()
                + (2.0 * PI * 277.0 * t).sin()
                + (2.0 * PI * 330.0 * t).sin());
    }
    for xi in x.iter_mut() {
        *xi += 0.05 * rng.standard_normal();
    }
    let beat = 60.0 / bpm;

    let kick: Vec<f64> = (0..(0.15 * sr as f64) as usize)
        .map(|j| (2.0 * PI * 60.0 * j as f64 / sr as f64).sin())
        .collect();
    let noise: Vec<f64> = (0..(0.2 * sr as f64) as usize)
        .map(|_| rng.standard_normal())
        .collect();

    let mut k = 0usize;
    while k as f64 * beat / 2.0 < duration_s {
        let at = k as f64 * beat / 2.0;
        hit(&mut x, sr, at, 0.05, &noise, 0.3); // hat
        if k.is_multiple_of(2) {
            hit(&mut x, sr, at, 0.15, &kick, 1.0);
            if (k / 2) % 2 == 1 {
                hit(&mut x, sr, at, 0.2, &noise, 0.8); // snare
            }
        }
        k += 1;
    }
    let max_abs = x.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    // (x / np.max(np.abs(x))).astype(np.float32)
    x.into_iter()
        .map(|v| ((v / max_abs) as f32) as f64)
        .collect()
}

/// `lambda: box[0]` - a clock reading from a mutable box.
fn clock(cell: &Rc<RefCell<f64>>) -> impl FnMut() -> f64 {
    let cell = Rc::clone(cell);
    move || *cell.borrow()
}

// -- rms_windows / pick_onsets ------------------------------------------

#[test]
fn test_rms_windows_empty_input() {
    assert!(rms_windows(&[], 22050).is_empty());
}

#[test]
fn test_pick_onsets_finds_roughly_the_right_count() {
    let samplerate = 22050;
    let track = click_track(120.0, 8.0, samplerate, 0.0);
    let rms = rms_windows_with(&track, samplerate, 20);
    let onsets = pick_onsets(&rms, 20.0 / 1000.0);
    // 120 BPM over 8s = 16 beats - allow slack for edge effects.
    assert!((12..=20).contains(&onsets.len()));
}

#[test]
fn test_pick_onsets_silence_finds_nothing() {
    let samplerate = 22050;
    let track = vec![0.0; (4.0 * samplerate as f64) as usize];
    let rms = rms_windows(&track, samplerate);
    assert!(pick_onsets(&rms, 0.02).is_empty());
}

#[test]
fn test_inter_onset_intervals_basic() {
    assert_eq!(
        inter_onset_intervals(&[0.0, 0.5, 1.0, 1.5]),
        vec![0.5, 0.5, 0.5]
    );
}

#[test]
fn test_inter_onset_intervals_ignores_non_increasing() {
    assert_eq!(inter_onset_intervals(&[0.0, 0.5, 0.3, 1.0]), vec![0.5, 0.7]);
}

// -- estimate_tempo -------------------------------------------------------

#[test]
fn test_estimate_tempo_none_with_too_few_onsets() {
    assert!(estimate_tempo(&[0.0, 0.5, 1.0]).is_none());
}

#[test]
fn test_estimate_tempo_recovers_known_bpm_from_clean_onsets() {
    let interval = 60.0 / 128.0;
    let onsets: Vec<f64> = (0..20).map(|i| i as f64 * interval).collect();
    let result = estimate_tempo(&onsets);
    assert!(result.is_some());
    let (bpm, confidence) = result.unwrap();
    assert!((bpm - 128.0).abs() < 1.0);
    assert!(confidence > 0.9);
}

#[test]
fn test_estimate_tempo_tolerates_light_jitter() {
    let mut rng = Rng::new(1);
    let interval = 60.0 / 100.0;
    let mut onsets: Vec<f64> = (0..24)
        .map(|i| i as f64 * interval + rng.uniform(-0.01, 0.01))
        .collect();
    onsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let result = estimate_tempo(&onsets);
    assert!(result.is_some());
    let (bpm, confidence) = result.unwrap();
    assert!((bpm - 100.0).abs() < 3.0);
    assert!(confidence > 0.7);
}

#[test]
fn test_estimate_tempo_from_a_real_click_track_end_to_end() {
    let samplerate = 22050;
    let track = click_track(140.0, 8.0, samplerate, 0.0);
    let rms = rms_windows_with(&track, samplerate, 20);
    let onsets = pick_onsets(&rms, 20.0 / 1000.0);
    let result = estimate_tempo(&onsets);
    assert!(result.is_some());
    let (bpm, _confidence) = result.unwrap();
    assert!((bpm - 140.0).abs() < 5.0);
}

#[test]
fn test_estimate_tempo_out_of_range_intervals_are_dropped() {
    // all gaps well outside the 50-200 BPM window (too slow: ~20 BPM)
    assert!(estimate_tempo(&[0.0, 3.0, 6.0, 9.0, 12.0]).is_none());
}

// -- beat_phase -------------------------------------------------------

#[test]
fn test_beat_phase_at_the_reference_time_is_zero() {
    assert_eq!(beat_phase(10.0, 10.0, 120.0), 0.0);
}

#[test]
fn test_beat_phase_halfway_through_a_beat() {
    let interval = 60.0 / 120.0;
    let phase = beat_phase(10.0 + interval / 2.0, 10.0, 120.0);
    assert!((phase - 0.5).abs() < 1e-9);
}

#[test]
fn test_beat_phase_wraps_around() {
    let interval = 60.0 / 120.0;
    let phase = beat_phase(10.0 + interval * 3.25, 10.0, 120.0);
    assert!((phase - 0.25).abs() < 1e-9);
}

#[test]
fn test_beat_phase_zero_bpm_is_safe() {
    assert_eq!(beat_phase(5.0, 0.0, 0.0), 0.0);
}

// -- TempoLock ------------------------------------------------------------

#[test]
fn test_lock_not_engaged_before_the_hold_time() {
    let cell = Rc::new(RefCell::new(0.0));
    let mut lock = TempoLock::with_clock(clock(&cell));
    assert!(!lock.update(Some((120.0, 0.9)), Some(0.0)));
    *cell.borrow_mut() = 2.0;
    assert!(!lock.update(Some((120.0, 0.9)), Some(2.0)));
}

#[test]
fn test_lock_engages_once_stable_for_the_hold_time() {
    let cell = Rc::new(RefCell::new(0.0));
    let mut lock = TempoLock::with_clock(clock(&cell));
    let mut result = false;
    for t in [0.0, 1.0, 2.0, 3.0, 4.0, 5.0] {
        result = lock.update(Some((120.0, 0.9)), Some(t));
    }
    assert!(result);
    assert!(lock.locked);
    assert_eq!(lock.bpm, 120.0);
}

#[test]
fn test_lock_breaks_on_a_sustained_bpm_change() {
    let cell = Rc::new(RefCell::new(0.0));
    let mut lock = TempoLock::with_clock(clock(&cell));
    for t in [0.0, 1.0, 2.0, 3.0, 4.0, 5.0] {
        lock.update(Some((120.0, 0.9)), Some(t));
    }
    assert!(lock.locked);

    // A different song: after a few ticks fewer than half the window
    // still agrees with 120 -> unlocked.
    let results: Vec<bool> = [6.0, 7.0, 8.0, 9.0]
        .iter()
        .map(|&t| lock.update(Some((200.0, 0.9)), Some(t)))
        .collect();
    assert!(results[0]); // one stray tick doesn't break it
    assert!(!results[results.len() - 1]);
}

#[test]
fn test_lock_tolerates_a_small_jump_within_tolerance() {
    let cell = Rc::new(RefCell::new(0.0));
    let mut lock = TempoLock::with_clock(clock(&cell));
    for t in [0.0, 1.0, 2.0, 3.0, 4.0] {
        lock.update(Some((120.0, 0.9)), Some(t));
    }
    // +3% - within BPM_TOLERANCE (4%), still agrees.
    let small_jump = 120.0 * (1.0 + BPM_TOLERANCE * 0.75);
    let result = lock.update(Some((small_jump, 0.9)), Some(5.0));
    assert!(result);
}

#[test]
fn test_a_single_low_confidence_or_missing_tick_does_not_prevent_or_break_a_lock() {
    let mut lock = TempoLock::new();
    for t in [0.0, 1.0, 2.0, 3.0, 4.0] {
        lock.update(Some((120.0, 0.9)), Some(t));
    }
    assert!(lock.update(Some((120.0, 0.01)), Some(5.0))); // 5 of 6 agree
    assert!(lock.update(None, Some(6.0)));
}

#[test]
fn test_music_stopping_unlocks_within_a_few_seconds() {
    let mut lock = TempoLock::new();
    for t in 0..8 {
        lock.update(Some((120.0, 0.9)), Some(t as f64));
    }
    assert!(lock.locked);
    let states: Vec<bool> = (8..14).map(|t| lock.update(None, Some(t as f64))).collect();
    assert!(states[0]);
    assert!(!states[states.len() - 1]);
    assert!(states.iter().position(|&s| !s).unwrap() <= 4);
}

#[test]
fn test_scattered_estimates_never_lock() {
    let mut lock = TempoLock::new();
    let bpms = [
        70.0, 140.0, 95.0, 180.0, 110.0, 64.0, 150.0, 88.0, 123.0, 77.0,
    ];
    assert!(
        bpms.iter()
            .enumerate()
            .all(|(t, &b)| !lock.update(Some((b, 0.9)), Some(t as f64)))
    );
}

#[test]
fn test_low_confidence_stream_never_locks() {
    let mut lock = TempoLock::new();
    assert!((0..10).all(|t| !lock.update(Some((120.0, 0.05)), Some(t as f64))));
}

#[test]
fn test_lock_reset_method() {
    let mut lock = TempoLock::new();
    lock.update(Some((120.0, 0.9)), Some(0.0));
    lock.reset();
    assert!(!lock.locked);
    assert_eq!(lock.bpm, 0.0);
}

#[test]
fn test_hold_time_constant_matches_wp_spec() {
    assert_eq!(LOCK_HOLD_S, 5.0);
}

// -- spectral-flux + autocorrelation estimator (audit 2026-09-24) -------------

#[test]
fn test_acf_estimator_finds_the_tempo_of_a_dense_mix() {
    for bpm in [96.0, 120.0, 128.0, 140.0] {
        let result = estimate_tempo_from_audio(&drum_pattern(bpm, 8.0, 48000, 0), 48000);
        assert!(result.is_some());
        let (estimated, confidence) = result.unwrap();
        assert!(
            (estimated - bpm).abs() / bpm < BPM_TOLERANCE,
            "{bpm}, {estimated}"
        );
        assert!(confidence >= 0.15);
    }
}

#[test]
fn test_acf_estimator_has_low_confidence_on_noise() {
    let mut rng = Rng::new(1);
    let noise: Vec<f64> = (0..8 * 48000)
        .map(|_| ((rng.standard_normal() as f32) * 0.1f32) as f64)
        .collect();
    let result = estimate_tempo_from_audio(&noise, 48000);
    assert!(result.is_none_or(|(_bpm, confidence)| confidence < 0.15));
}

#[test]
fn test_live_loop_simulation_locks_within_the_hold_time() {
    let sr = 48000u32;
    let track = drum_pattern(110.0, 20.0, sr, 3);
    let mut lock = TempoLock::new();
    let mut locked_at: Option<usize> = None;
    for t in 8usize..20 {
        let estimate =
            estimate_tempo_from_audio(&track[(t - 8) * sr as usize..t * sr as usize], sr);
        if lock.update(estimate, Some(t as f64)) && locked_at.is_none() {
            // `locked_at = locked_at or t` - first locked tick wins.
            locked_at = Some(t);
        }
    }
    assert!(locked_at.is_some_and(|t| t <= 14));
    assert!((lock.bpm - 110.0).abs() < 4.4);
}
