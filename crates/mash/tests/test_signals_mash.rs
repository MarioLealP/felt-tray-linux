//! signals.mash (WP-17): MashDetector's pure sustained-rate logic over
//! synthetic events/s readings; EventCounter's counting behaviour (not its
//! real pynput wiring, which needs a live hook).

use std::cell::Cell;

use felt_mash::{EventCounter, MashDetector};

// -- MashDetector -----------------------------------------------------------

#[test]
fn test_no_fire_below_the_rate_threshold() {
    let clock = Cell::new(0.0);
    let mut detector = MashDetector::new(8.0, 3.0, felt_mash::MASH_COOLDOWN_S, || clock.get());
    for _ in 0..5 {
        assert!(!detector.update(5.0));
    }
}

#[test]
fn test_no_fire_until_sustained_for_the_full_duration() {
    let clock = Cell::new(0.0);
    let mut detector = MashDetector::new(8.0, 3.0, felt_mash::MASH_COOLDOWN_S, || clock.get());

    assert!(!detector.update(10.0)); // t=0, just crossed
    clock.set(1.0);
    assert!(!detector.update(10.0)); // t=1, only 1s sustained
    clock.set(2.0);
    assert!(!detector.update(10.0)); // t=2, only 2s sustained
    clock.set(3.0);
    assert!(detector.update(10.0)); // t=3, sustained the full 3s
}

#[test]
fn test_a_brief_dip_below_threshold_resets_the_sustain_timer() {
    let clock = Cell::new(0.0);
    let mut detector = MashDetector::new(8.0, 3.0, felt_mash::MASH_COOLDOWN_S, || clock.get());

    detector.update(10.0);
    clock.set(2.0);
    detector.update(10.0); // 2s sustained so far - one more second would fire
    clock.set(2.5);
    assert!(!detector.update(3.0)); // dipped below threshold - resets the clock
    clock.set(3.0);
    assert!(!detector.update(10.0)); // back above threshold, but the sustain re-anchored here
    clock.set(5.9);
    assert!(!detector.update(10.0)); // only 2.9s since the re-anchor at t=3.0
    clock.set(6.0);
    assert!(detector.update(10.0)); // now a full 3.0s since the re-anchor
}

/// Python: `_mash_until_it_fires(detector, clock, start)`.
fn mash_until_it_fires(
    detector: &mut MashDetector<impl FnMut() -> f64>,
    clock: &Cell<f64>,
    start: f64,
) {
    clock.set(start);
    detector.update(10.0);
    clock.set(start + 3.0);
    assert!(detector.update(10.0));
}

#[test]
fn test_does_not_refire_within_the_cooldown() {
    let clock = Cell::new(0.0);
    let mut detector = MashDetector::new(8.0, 3.0, 5.0, || clock.get());
    mash_until_it_fires(&mut detector, &clock, 0.0); // fires at t=3.0

    clock.set(4.0);
    assert!(!detector.update(10.0)); // still mashing, but within cooldown
}

#[test]
fn test_fires_again_after_the_cooldown_elapses() {
    let clock = Cell::new(0.0);
    let mut detector = MashDetector::new(8.0, 3.0, 5.0, || clock.get());
    mash_until_it_fires(&mut detector, &clock, 0.0); // fires at t=3.0

    clock.set(8.0);
    assert!(detector.update(10.0)); // 5s cooldown elapsed, still mashing
}

// -- EventCounter -------------------------------------------------------------

#[test]
fn test_event_counter_counts_clicks_only_on_press_not_release() {
    let counter = EventCounter::new();
    counter.on_click(0, 0, "left", true);
    counter.on_click(0, 0, "left", false);
    counter.on_click(0, 0, "left", true);

    assert_eq!(counter.take_count(), 2);
}

#[test]
fn test_event_counter_has_no_keyboard_hook() {
    // Audit 2026-09-24: typing must never count toward "mash".
    // Python asserts `not hasattr(counter, "_on_key")`. Rust has no runtime
    // attribute lookup: the guarantee here is structural (EventCounter has
    // no key-callback method or field), checked over the stored fields the
    // way `vars(counter)` is walked in the Python tests.
    let counter = EventCounter::new();
    let stored_values = format!("{counter:?}");
    assert!(!stored_values.contains("key"));
    assert!(!stored_values.contains("Key"));
}

#[test]
fn test_event_counter_take_count_resets_to_zero() {
    let counter = EventCounter::new();
    counter.on_click(0, 0, "left", true);
    counter.take_count();

    assert_eq!(counter.take_count(), 0);
}

#[test]
fn test_event_counter_never_stores_what_was_pressed() {
    // The actual privacy guarantee: there is no attribute anywhere on
    // EventCounter holding key/button identity after a callback runs -
    // only the integer count exists.
    let counter = EventCounter::new();
    counter.on_click(123, 456, "right", true);

    let stored_values = format!("{counter:?}");
    assert!(!stored_values.contains("123") && !stored_values.contains("right"));
    assert!(!stored_values.contains('[')); // no list/tuple attrs
}
