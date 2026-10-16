//! signals.call_energy (WP-17): pure smoothing/threshold logic over
//! synthetic peak/talking samples, no real audio capture involved.

use std::cell::RefCell;
use std::rc::Rc;

use felt_call_energy::{
    CallEnergyTracker, CallJoinGuessTracker, ENERGY_SMOOTHING, JOIN_GUESS_SILENCE_S,
    LIVELY_PEAK_THRESHOLD, LIVELY_SUSTAIN_S, QUIET_HANG_S,
};

/// Shared mutable clock cell, mirroring the Python tests' `clock = [0.0]`
/// idiom: tests mutate the cell to advance time.
fn fake_clock() -> (Rc<RefCell<f64>>, impl FnMut() -> f64) {
    let cell = Rc::new(RefCell::new(0.0));
    let reader = Rc::clone(&cell);
    (cell, move || *reader.borrow())
}

// -- CallEnergyTracker --------------------------------------------------

#[test]
fn test_no_lively_event_on_low_peaks() {
    let (clock, now_fn) = fake_clock();
    // Python: lively_threshold=0.08, sustain_s=5.0; quiet_hang_s/smoothing defaulted.
    let mut tracker = CallEnergyTracker::new(
        LIVELY_PEAK_THRESHOLD,
        LIVELY_SUSTAIN_S,
        QUIET_HANG_S,
        ENERGY_SMOOTHING,
        now_fn,
    );
    for i in 0..10 {
        *clock.borrow_mut() = i as f64;
        assert!(tracker.update(0.01).is_none());
    }
}

#[test]
fn test_lively_fires_after_sustained_high_energy() {
    let (clock, now_fn) = fake_clock();
    // Python: smoothing=1.0; quiet_hang_s defaulted.
    let mut tracker = CallEnergyTracker::new(
        LIVELY_PEAK_THRESHOLD,
        LIVELY_SUSTAIN_S,
        QUIET_HANG_S,
        1.0,
        now_fn,
    );

    assert!(tracker.update(0.5).is_none()); // smoothed jumps straight to 0.5 with smoothing=1.0, t=0
    *clock.borrow_mut() = 4.9;
    assert!(tracker.update(0.5).is_none()); // only 4.9s sustained
    *clock.borrow_mut() = 5.0;
    assert_eq!(tracker.update(0.5), Some("lively"));
}

#[test]
fn test_quiet_fires_after_energy_drops_and_hangs() {
    let (clock, now_fn) = fake_clock();
    let mut tracker = CallEnergyTracker::new(
        LIVELY_PEAK_THRESHOLD,
        LIVELY_SUSTAIN_S,
        QUIET_HANG_S,
        1.0,
        now_fn,
    );
    let _ = tracker.update(0.5);
    *clock.borrow_mut() = 5.0;
    assert_eq!(tracker.update(0.5), Some("lively"));

    *clock.borrow_mut() = 6.0;
    assert!(tracker.update(0.0).is_none()); // dropped, but hasn't hung 8s yet
    *clock.borrow_mut() = 14.0;
    assert_eq!(tracker.update(0.0), Some("quiet"));
}

#[test]
fn test_quiet_does_not_fire_on_a_brief_dip() {
    let (clock, now_fn) = fake_clock();
    let mut tracker = CallEnergyTracker::new(
        LIVELY_PEAK_THRESHOLD,
        LIVELY_SUSTAIN_S,
        QUIET_HANG_S,
        1.0,
        now_fn,
    );
    let _ = tracker.update(0.5);
    *clock.borrow_mut() = 5.0;
    let _ = tracker.update(0.5); // lively

    *clock.borrow_mut() = 6.0;
    assert!(tracker.update(0.0).is_none()); // brief dip
    *clock.borrow_mut() = 7.0;
    assert!(
        tracker.update(0.5).is_none() // back up before the hang elapsed - stays lively, no quiet fired
    );
}

// -- CallJoinGuessTracker -------------------------------------------------

#[test]
fn test_no_fire_on_the_very_first_talking_sample() {
    // Python omitted now_fn (default time.monotonic); the clock value is
    // never load-bearing in this test, so a fixed clock is equivalent.
    let mut tracker = CallJoinGuessTracker::new(JOIN_GUESS_SILENCE_S, || 0.0);
    assert!(!tracker.update(true));
}

#[test]
fn test_no_fire_after_a_short_pause() {
    let (clock, now_fn) = fake_clock();
    let mut tracker = CallJoinGuessTracker::new(JOIN_GUESS_SILENCE_S, now_fn);
    tracker.update(true);
    *clock.borrow_mut() = 10.0;
    tracker.update(false);
    *clock.borrow_mut() = 30.0;
    assert!(!tracker.update(true)); // only 20s of silence
}

#[test]
fn test_fires_after_a_long_silence_then_talking_resumes() {
    let (clock, now_fn) = fake_clock();
    let mut tracker = CallJoinGuessTracker::new(JOIN_GUESS_SILENCE_S, now_fn);
    tracker.update(true);
    *clock.borrow_mut() = 10.0;
    tracker.update(false);
    *clock.borrow_mut() = 140.0; // 130s of silence
    assert!(tracker.update(true));
}

#[test]
fn test_does_not_refire_while_still_talking() {
    let (clock, now_fn) = fake_clock();
    let mut tracker = CallJoinGuessTracker::new(JOIN_GUESS_SILENCE_S, now_fn);
    tracker.update(true);
    *clock.borrow_mut() = 10.0;
    tracker.update(false);
    *clock.borrow_mut() = 140.0;
    assert!(tracker.update(true));
    *clock.borrow_mut() = 145.0;
    assert!(!tracker.update(true)); // still talking, not a new burst
}
