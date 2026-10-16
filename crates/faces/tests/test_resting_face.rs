//! WP-15 "resting face follows mood" (audit 2026-09-24): the pure
//! `faces.resting_face_for_mood` mapping. The send-on-change
//! `{"resting_face": {...}}` wiring was moved to behavior.py
//! (`update_resting_face`/`resend_resting_face`) by WP-40's god-file split -
//! the two Python tests that exercise that wiring
//! (`test_resting_face_is_sent_only_on_change`,
//! `test_undelivered_resting_face_is_retried`) are not ported here because
//! they test behavior.py, not the faces module.

use std::collections::HashMap;

use felt_faces::resting_face_for_mood;

/// `_BASE` mood from the Python tests (affection is carried but unused by the
/// mapping, same as Python).
fn base() -> HashMap<String, f64> {
    HashMap::from([
        ("energy".to_string(), 0.6),
        ("happiness".to_string(), 0.5),
        ("boredom".to_string(), 0.1),
        ("affection".to_string(), 0.5),
        ("annoyance".to_string(), 0.0),
    ])
}

/// Python's `{**_BASE, key: value}`.
fn with(mood: &HashMap<String, f64>, key: &str, value: f64) -> HashMap<String, f64> {
    let mut mood = mood.clone();
    mood.insert(key.to_string(), value);
    mood
}

#[test]
fn test_neutral_mood_rests_neutral() {
    assert!(resting_face_for_mood(&base()).is_empty());
}

#[test]
fn test_content_gets_a_faint_soft_smile() {
    let face = resting_face_for_mood(&with(&base(), "happiness", 0.7));
    let mth_up = face["Fcl_MTH_Up"];
    assert!(0.0 < mth_up && mth_up <= 0.15);
    let eye_joy = face["Fcl_EYE_Joy"];
    assert!(0.0 < eye_joy && eye_joy <= 0.1);
}

#[test]
fn test_low_energy_and_boredom_lower_the_lids() {
    let sleepy = resting_face_for_mood(&with(&base(), "energy", 0.1));
    let bored = resting_face_for_mood(&with(&base(), "boredom", 0.9));
    assert!(sleepy["Fcl_EYE_Close"] > 0.15);
    let close = bored["Fcl_EYE_Close"];
    assert!(0.0 < close && close < 0.25);
}

#[test]
fn test_annoyance_wins_over_the_smile() {
    let mut mood = with(&base(), "happiness", 0.9);
    mood.insert("annoyance".to_string(), 0.8);
    let face = resting_face_for_mood(&mood);
    assert!(face["Fcl_BRW_Angry"] > 0.1);
    assert!(!face.contains_key("Fcl_MTH_Up"));
}
