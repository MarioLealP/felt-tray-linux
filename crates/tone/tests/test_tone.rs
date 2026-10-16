//! tone.rs (WP-15): time-band selection, mood modifiers, and streak
//! escalation. Pure functions, no clock involved.

use felt_tone::{DAY, EVENING, LATE_NIGHT, MORNING, Mood, build_tone_line, time_band};
use std::collections::BTreeMap;

fn neutral_mood() -> Mood {
    let mut mood = BTreeMap::new();
    mood.insert("energy".to_string(), 0.5);
    mood.insert("happiness".to_string(), 0.5);
    mood.insert("boredom".to_string(), 0.3);
    mood.insert("affection".to_string(), 0.5);
    mood.insert("annoyance".to_string(), 0.0);
    mood
}

/// Python `{**_NEUTRAL_MOOD, ...overrides}`.
fn mood_with(overrides: &[(&str, f64)]) -> Mood {
    let mut mood = neutral_mood();
    for (key, value) in overrides {
        mood.insert((*key).to_string(), *value);
    }
    mood
}

// -- time_band --------------------------------------------------------------

#[test]
fn test_time_band_morning() {
    assert_eq!(time_band(9), MORNING);
}

#[test]
fn test_time_band_day() {
    assert_eq!(time_band(14), DAY);
}

#[test]
fn test_time_band_evening() {
    assert_eq!(time_band(20), EVENING);
}

#[test]
fn test_time_band_late_night_before_midnight() {
    assert_eq!(time_band(23), LATE_NIGHT);
}

#[test]
fn test_time_band_late_night_after_midnight() {
    assert_eq!(time_band(2), LATE_NIGHT);
}

#[test]
fn test_time_band_boundary_four_am_is_morning() {
    assert_eq!(time_band(4), MORNING);
}

// -- build_tone_line: baseline ----------------------------------------------

#[test]
fn test_neutral_mood_gets_only_the_baseline_band_line() {
    let line = build_tone_line(&neutral_mood(), 14, None);
    assert!(line.contains("Hype-friend"));
    assert!(!line.contains("annoyed"));
    assert!(!line.contains("sleepy"));
}

// -- mood modifiers -----------------------------------------------------

#[test]
fn test_annoyed_mood_adds_the_short_sarcastic_modifier() {
    let mood = mood_with(&[("annoyance", 0.8)]);
    let line = build_tone_line(&mood, 14, None);
    assert!(line.contains("sarcastic"));
}

#[test]
fn test_low_energy_adds_the_sleepy_modifier() {
    let mood = mood_with(&[("energy", 0.1)]);
    let line = build_tone_line(&mood, 14, None);
    assert!(line.contains("sleepy"));
}

#[test]
fn test_high_boredom_adds_the_heckling_modifier() {
    let mood = mood_with(&[("boredom", 0.9)]);
    let line = build_tone_line(&mood, 14, None);
    assert!(line.contains("heckling"));
}

#[test]
fn test_high_happiness_adds_the_good_mood_modifier() {
    let mood = mood_with(&[("happiness", 0.9)]);
    let line = build_tone_line(&mood, 14, None);
    assert!(line.contains("good mood"));
}

#[test]
fn test_multiple_active_mood_axes_all_show_up() {
    let mood = mood_with(&[("annoyance", 0.8), ("boredom", 0.9)]);
    let line = build_tone_line(&mood, 14, None);
    assert!(line.contains("sarcastic"));
    assert!(line.contains("heckling"));
}

// -- streak escalation --------------------------------------------------

#[test]
fn test_single_negative_reaction_gets_no_streak_line() {
    let line = build_tone_line(&neutral_mood(), 14, Some(&["sad"]));
    assert!(!line.contains("row"));
}

#[test]
fn test_second_negative_reaction_in_a_row_switches_register() {
    let line = build_tone_line(&neutral_mood(), 14, Some(&["sad", "annoyed"]));
    assert!(line.contains("switch it up"));
}

#[test]
fn test_third_negative_reaction_in_a_row_drops_the_grump() {
    let line = build_tone_line(&neutral_mood(), 14, Some(&["sad", "sad", "annoyed"]));
    assert!(line.contains("drop it entirely"));
}

#[test]
fn test_streak_breaks_on_a_positive_reaction() {
    let line = build_tone_line(&neutral_mood(), 14, Some(&["sad", "sad", "happy"]));
    assert!(!line.contains("row"));
}

#[test]
fn test_upbeat_run_damps_instead_of_escalating() {
    // B-1: her own excitement is not a win; a run of it must never tell the
    // model to hype harder or claim wins happened.
    for streak in [
        &["happy", "excited"][..],
        &["happy", "excited", "happy", "excited"][..],
    ] {
        let line = build_tone_line(&neutral_mood(), 14, Some(streak));
        assert!(line.contains("not more hype") || line.contains("no cheering"));
        assert!(!line.to_lowercase().contains("win"));
    }
}

#[test]
fn test_no_tone_line_ever_asks_for_more_cheering() {
    let mood = mood_with(&[("happiness", 0.9)]);
    let line = build_tone_line(&mood, 14, Some(&["excited", "excited", "excited"]));
    assert!(!line.contains("cheering and"));
    assert!(!line.contains("go big"));
}

#[test]
fn test_empty_streak_is_fine() {
    let line = build_tone_line(&neutral_mood(), 14, Some(&[]));
    assert!(!line.contains("row"));
}
