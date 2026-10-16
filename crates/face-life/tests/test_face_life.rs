//! face_life.py (WP-12): micro-expression picking (no immediate repeats)
//! and speech-envelope peak detection (pure signal processing, no real audio
//! device or overlay involved).

use felt_face_life::{
    MICRO_EXPRESSION_POOL, PEAK_REFRACTORY_S, Rng, detect_envelope_peaks, pick_micro_expression,
};

// -- pick_micro_expression --------------------------------------------

/// Deterministic SplitMix64 generator so the picking tests are reproducible.
struct TestRng(u64);

impl TestRng {
    fn with_seed(seed: u64) -> Self {
        TestRng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Rng for TestRng {
    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }
}

#[test]
fn test_never_repeats_the_last_pick_immediately() {
    let mut rng = TestRng::with_seed(1);
    for last in MICRO_EXPRESSION_POOL {
        for _ in 0..20 {
            assert_ne!(pick_micro_expression(&mut rng, Some(last)), last);
        }
    }
}

#[test]
fn test_first_pick_with_no_history_is_a_valid_pose() {
    let mut rng = TestRng::with_seed(2);
    assert!(MICRO_EXPRESSION_POOL.contains(&pick_micro_expression(&mut rng, None)));
}

#[test]
fn test_eventually_covers_the_whole_pool() {
    let mut rng = TestRng::with_seed(3);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..200 {
        seen.insert(pick_micro_expression(&mut rng, None));
    }
    let pool: std::collections::BTreeSet<&str> = MICRO_EXPRESSION_POOL.into_iter().collect();
    assert_eq!(seen, pool);
}

// -- detect_envelope_peaks -----------------------------------------------

fn silence(seconds: f64, samplerate: u32) -> Vec<i16> {
    vec![0; (seconds * f64::from(samplerate)) as usize]
}

fn tone(seconds: f64, amplitude: i32, samplerate: u32) -> Vec<i16> {
    let n = (seconds * f64::from(samplerate)) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(samplerate);
            (f64::from(amplitude) * (2.0 * std::f64::consts::PI * 220.0 * t).sin()) as i16
        })
        .collect()
}

#[test]
fn test_pure_silence_has_no_peaks() {
    let frames = silence(1.0, 16000);
    assert!(detect_envelope_peaks(&frames, 1, 16000).is_empty());
}

#[test]
fn test_empty_audio_has_no_peaks() {
    let empty: Vec<i16> = Vec::new();
    assert!(detect_envelope_peaks(&empty, 1, 16000).is_empty());
}

#[test]
fn test_a_loud_burst_in_quiet_audio_is_detected_as_a_peak() {
    let samplerate = 16000;
    let quiet = tone(1.0, 200, samplerate);
    let loud = tone(0.2, 20000, samplerate);
    let mut frames = quiet[..samplerate as usize / 2].to_vec();
    frames.extend_from_slice(&loud);
    frames.extend_from_slice(&quiet[samplerate as usize / 2..]);

    let peaks = detect_envelope_peaks(&frames, 1, samplerate);

    assert!(!peaks.is_empty());
    // The loud burst starts at ~0.5s - every detected peak should fall
    // somewhere near/inside that burst, not scattered across the quiet part.
    assert!(peaks.iter().all(|p| (0.4..=0.8).contains(p)));
}

#[test]
fn test_uniform_volume_audio_has_no_peaks() {
    let frames = tone(1.0, 5000, 16000);
    assert!(detect_envelope_peaks(&frames, 1, 16000).is_empty());
}

#[test]
fn test_stereo_audio_is_averaged_across_channels() {
    let samplerate = 16000;
    let mono = tone(1.0, 5000, samplerate);
    let stereo: Vec<i16> = mono.iter().flat_map(|s| [*s, *s]).collect();
    // Should behave the same as the mono version - no crash, consistent shape handling.
    let stereo_peaks = detect_envelope_peaks(&stereo, 2, samplerate);
    let mono_peaks = detect_envelope_peaks(&mono, 1, samplerate);
    assert_eq!(stereo_peaks, mono_peaks);
}

#[test]
fn test_peak_times_are_within_audio_duration() {
    let samplerate = 16000;
    let mut frames = tone(0.5, 100, samplerate);
    frames.extend(tone(0.1, 20000, samplerate));
    frames.extend(tone(0.5, 100, samplerate));
    let duration_s = frames.len() as f64 / f64::from(samplerate);

    let peaks = detect_envelope_peaks(&frames, 1, samplerate);

    assert!(peaks.iter().all(|p| (0.0..=duration_s).contains(p)));
}

// -- audit 2026-09-24: rolling mean, one peak per burst, refractory ------------

#[test]
fn test_one_loud_burst_gives_exactly_one_peak() {
    let samplerate = 16000;
    let mut frames = tone(1.0, 300, samplerate);
    frames.extend(tone(0.3, 20000, samplerate));
    frames.extend(tone(1.0, 300, samplerate));
    assert_eq!(detect_envelope_peaks(&frames, 1, samplerate).len(), 1);
}

#[test]
fn test_peaks_respect_the_refractory_gap() {
    let samplerate = 16000;
    let mut parts = Vec::new();
    for _ in 0..12 {
        // syllable-like bursts every 0.2 s
        parts.extend(tone(0.1, 300, samplerate));
        parts.extend(tone(0.1, 15000, samplerate));
    }
    let peaks = detect_envelope_peaks(&parts, 1, samplerate);
    assert!(!peaks.is_empty());
    // peaks.windows(2) pairs consecutive peaks, like zip(peaks, peaks[1:]).
    assert!(
        peaks
            .windows(2)
            .all(|w| w[1] - w[0] >= PEAK_REFRACTORY_S - 1e-9)
    );
    const { assert!(PEAK_REFRACTORY_S >= 0.45) }; // at least main.gd's own accent refractory
}

#[test]
fn test_rolling_mean_adapts_to_a_louder_second_half() {
    let samplerate = 16000;
    let mut frames = tone(1.5, 500, samplerate);
    frames.extend(tone(3.0, 8000, samplerate));
    // One step up in level is a single accent, not a peak on every window
    // of the loud half (the old whole-clip mean flagged all of them).
    assert!(detect_envelope_peaks(&frames, 1, samplerate).len() <= 1);
}
