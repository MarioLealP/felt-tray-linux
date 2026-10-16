//! MoodState (WP-15): daily energy curve, boredom rise, annoyance decay,
//! event nudges, and the once-per-day partial reset. Clock-injected (both the
//! monotonic `now_fn` for elapsed-time math and a wall-clock `clock_fn` for
//! hour/date), no real sleeps.

use std::cell::Cell;
use std::collections::BTreeMap;

use felt_mood::{DateTime, MoodState, drift_annoyance, drift_boredom, drift_energy};

/// Mirrors the Python `_clocks` helper: read-only views over mutable boxes
/// the test pokes between ticks.
fn _clocks<'a>(
    now_box: &'a Cell<f64>,
    dt_box: &'a Cell<DateTime>,
) -> (impl Fn() -> f64 + 'a, impl Fn() -> DateTime + 'a) {
    (move || now_box.get(), move || dt_box.get())
}

// -- pure drift functions --------------------------------------------------

#[test]
fn test_energy_drifts_toward_the_hour_target() {
    // hour 14 (2pm) targets 0.85 - starting low, energy should rise.
    let result = drift_energy(0.3, 14, 20.0 * 60.0);
    assert!(0.3 < result && result < 0.85);
}

#[test]
fn test_energy_drifts_down_toward_a_low_overnight_target() {
    let result = drift_energy(0.8, 3, 20.0 * 60.0);
    assert!(result < 0.8);
}

#[test]
fn test_energy_with_zero_elapsed_is_unchanged() {
    assert!(drift_energy(0.5, 14, 0.0) == 0.5);
}

#[test]
fn test_boredom_rises_over_time() {
    let result = drift_boredom(0.0, 45.0 * 60.0);
    assert!(result > 0.9); // ~1.0 over the ~45 min half-life-ish window
}

#[test]
fn test_boredom_never_exceeds_one() {
    assert!(drift_boredom(0.99, 1e6) == 1.0);
}

#[test]
fn test_annoyance_decays_toward_zero() {
    let result = drift_annoyance(1.0, 30.0 * 60.0);
    assert!(result < 0.5); // roughly one time-constant of decay
}

#[test]
fn test_annoyance_never_goes_negative() {
    assert!(drift_annoyance(0.0, 1000.0) == 0.0);
}

// -- MoodState.tick ---------------------------------------------------------

#[test]
fn test_tick_applies_drift_since_last_tick() {
    let now_box = Cell::new(0.0);
    let dt_box = Cell::new(DateTime::new(2026, 9, 23, 14));
    let (now_fn, clock_fn) = _clocks(&now_box, &dt_box);
    let mut mood = MoodState::new(
        Some(BTreeMap::from([("boredom".to_string(), 0.0)])),
        now_fn,
        clock_fn,
    );

    now_box.set(45.0 * 60.0);
    mood.tick();

    assert!(mood.values["boredom"] > 0.9);
}

#[test]
fn test_tick_with_no_elapsed_time_is_a_noop() {
    let now_box = Cell::new(0.0);
    let dt_box = Cell::new(DateTime::new(2026, 9, 23, 14));
    let (now_fn, clock_fn) = _clocks(&now_box, &dt_box);
    let mut mood = MoodState::new(
        Some(BTreeMap::from([("boredom".to_string(), 0.4)])),
        now_fn,
        clock_fn,
    );

    mood.tick();
    mood.tick();

    assert!(mood.values["boredom"] == 0.4);
}

// -- daily reset --------------------------------------------------------

#[test]
fn test_no_reset_on_the_same_day() {
    let now_box = Cell::new(0.0);
    let dt_box = Cell::new(DateTime::new(2026, 9, 23, 9));
    let (now_fn, clock_fn) = _clocks(&now_box, &dt_box);
    let mut mood = MoodState::new(
        Some(BTreeMap::from([
            ("annoyance".to_string(), 0.8),
            ("boredom".to_string(), 0.7),
        ])),
        now_fn,
        clock_fn,
    );
    mood.tick(); // adopts today as the baseline, no reset yet

    dt_box.set(DateTime::new(2026, 9, 23, 20));
    mood.tick();

    assert!(mood.values["annoyance"] > 0.0); // not reset - still the same day
}

#[test]
fn test_reset_fires_once_on_a_new_day() {
    let now_box = Cell::new(0.0);
    let dt_box = Cell::new(DateTime::new(2026, 9, 23, 23));
    let (now_fn, clock_fn) = _clocks(&now_box, &dt_box);
    let mut mood = MoodState::new(
        Some(BTreeMap::from([
            ("annoyance".to_string(), 0.8),
            ("boredom".to_string(), 0.9),
            ("affection".to_string(), 0.9),
        ])),
        now_fn,
        clock_fn,
    );
    mood.tick(); // adopts 2026-09-23 as the baseline day

    dt_box.set(DateTime::new(2026, 9, 24, 0));
    mood.tick();

    assert!(mood.values["annoyance"] == 0.0); // wiped outright
    assert!(mood.values["boredom"] < 0.9); // eased, not fully cleared
    assert!(mood.values["affection"] == 0.9); // never touched by the reset

    // doesn't refire again on the same new day
    mood.values.insert("annoyance".to_string(), 0.5);
    dt_box.set(DateTime::new(2026, 9, 24, 1));
    mood.tick();
    assert!(mood.values["annoyance"] == 0.5);
}

// -- event nudges ---------------------------------------------------------

#[test]
fn test_reaction_spoken_nudges_happiness_up_and_boredom_down() {
    let mut mood = MoodState::new(
        Some(BTreeMap::from([
            ("happiness".to_string(), 0.5),
            ("boredom".to_string(), 0.5),
        ])),
        || 0.0,
        || DateTime::new(2026, 9, 23, 14),
    );
    mood.on_reaction_spoken();
    assert!(mood.values["happiness"] > 0.5);
    assert!(mood.values["boredom"] < 0.5);
}

#[test]
fn test_hush_nudges_annoyance_up() {
    let mut mood = MoodState::new(
        Some(BTreeMap::from([("annoyance".to_string(), 0.0)])),
        || 0.0,
        || DateTime::new(2026, 9, 23, 14),
    );
    mood.on_hush();
    assert!(mood.values["annoyance"] > 0.0);
}

#[test]
fn test_talked_to_nudges_affection_happiness_up_and_boredom_down() {
    let mut mood = MoodState::new(
        Some(BTreeMap::from([
            ("affection".to_string(), 0.5),
            ("happiness".to_string(), 0.5),
            ("boredom".to_string(), 0.5),
        ])),
        || 0.0,
        || DateTime::new(2026, 9, 23, 14),
    );
    mood.on_talked_to();
    assert!(mood.values["affection"] > 0.5);
    assert!(mood.values["happiness"] > 0.5);
    assert!(mood.values["boredom"] < 0.5);
}

#[test]
fn test_nudges_clamp_at_one() {
    let mut mood = MoodState::new(
        Some(BTreeMap::from([("annoyance".to_string(), 0.99)])),
        || 0.0,
        || DateTime::new(2026, 9, 23, 14),
    );
    mood.on_hush();
    assert!(mood.values["annoyance"] == 1.0);
}

// -- debug override ---------------------------------------------------------

#[test]
fn test_set_debug_overrides_only_known_keys_and_clamps() {
    let mut mood = MoodState::new(None, || 0.0, || DateTime::new(2026, 9, 23, 14));
    mood.set_debug(&BTreeMap::from([
        ("boredom".to_string(), 1.5),
        ("not_a_real_key".to_string(), 99.0),
    ]));
    assert!(mood.values["boredom"] == 1.0);
    assert!(!mood.values.contains_key("not_a_real_key"));
}

#[test]
fn test_as_dict_is_a_copy_not_a_live_reference() {
    let mood = MoodState::new(None, || 0.0, || DateTime::new(2026, 9, 23, 14));
    let mut snapshot = mood.as_dict();
    snapshot.insert("energy".to_string(), 0.0);
    assert!(mood.values["energy"] != 0.0);
}
