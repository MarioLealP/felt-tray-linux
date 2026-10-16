//! WP-15: a small, data-driven, mood-aware picker. reactions.json maps an
//! event key to a weighted list of entries, the spec's {face, tag?, prop?,
//! speak?} shape:
//!
//! ```text
//! {"pose": <face_poses.json name>, "weight": n,
//!  "mood_boost": {mood_key: multiplier},   optional
//!  "tag": <animation tag>,                 optional body clip
//!  "speak": true|false,                    optional, default false
//!  "prop": <prop name>}                    optional, WP-21 - no-op for now
//! ```
//!
//! `tag` is validated against the real library by the caller (never trusted
//! blindly). `speak` means "this reaction also wants a spoken line" - for
//! signal events server.py then asks felt-core (POST /v1/react/event); for
//! reply:<emotion> events it's ignored, the reply already is speech.
//! server.py picks from here instead of a flat random choice, so boredom/
//! annoyance/etc. bias which reaction shows, with no immediate repeats per
//! event (audit 2026-09-24: reply picks passed no `last` before).
//!
//! Ported from `felt_animation/reaction_table.py` (WP-15). Entries stay raw
//! parsed JSON - like the Python `dict`s - so rows pass through the picker
//! untouched.

use std::collections::HashMap;
use std::path::Path;

use rand::{RngExt, rngs::StdRng};
use serde_json::Value;

/// One reaction-table row: the raw parsed JSON of one entry (always an
/// object in well-formed tables - the Python `dict`).
pub type Entry = Value;

/// Injected RNG - the Rust stand-in for the structural `RandomLike` protocol
/// from `felt_animation.randomness` (shared there by sounds.py,
/// reaction_table.py, lipsync.py and loops.py; only the two methods this
/// module actually calls are defined here). The Python accepts any object
/// with these methods (`random.Random` instances, the `random` module);
/// here implementors provide `uniform` and `choice` explicitly, and tests
/// inject a seeded `StdRng` for determinism.
pub trait RandomLike {
    /// Python's `random.uniform(a, b)`: `a + (b - a) * random()`.
    fn uniform(&mut self, a: f64, b: f64) -> f64;

    /// Python's `random.choice(seq)`: a uniformly random element of `seq`.
    fn choice<'a, T>(&mut self, seq: &'a [T]) -> &'a T;
}

impl RandomLike for StdRng {
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.random::<f64>()
    }

    fn choice<'a, T>(&mut self, seq: &'a [T]) -> &'a T {
        // Panics on an empty slice, like `random.choice([])` raising IndexError.
        &seq[self.random_range(0..seq.len())]
    }
}

/// Read the reactions table from `path`. Returns an empty table when the
/// file is missing or unparseable - the Python version logs and no-ops
/// ("reaction table picks will no-op"); this pure crate has no logger, so
/// it silently no-ops the same way. (The Python `load_reactions()` defaults
/// to the package-root reactions.json; here the path is always explicit.)
pub fn load_reactions(path: &Path) -> HashMap<String, Vec<Entry>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// What "the same reaction again" means for repeat-avoidance: the face
/// plus the body clip. Matches the Python `f"{entry.get('pose')}|{entry.get('tag') or ''}"`:
/// a missing (or null) pose formats as "None", a falsy tag as "".
pub fn entry_key(entry: &Entry) -> String {
    let pose = match entry.get("pose") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "None".to_owned(),
        Some(other) => other.to_string(),
    };
    let tag = entry.get("tag").and_then(Value::as_str).unwrap_or("");
    format!("{pose}|{tag}")
}

/// Weighted pick with mood multipliers and no-immediate-repeat (same
/// spirit as face_life.pick_micro_expression). Each entry's effective
/// weight is its base "weight" times, for every {mood_key: multiplier}
/// pair in its optional "mood_boost", a lerp between 1.0 (mood_key's
/// value == 0) and multiplier (mood_key's value == 1) - so e.g. a
/// gradually-rising boredom gradually favors a heckling entry rather than
/// snapping to it at some arbitrary threshold.
///
/// `last_pose` may be a bare pose name or an [`entry_key`] - either one
/// excludes the entry it names. `rng` is injected instead of the Python's
/// process-wide `random` default so tests are deterministic.
pub fn pick_reaction<'a, R: RandomLike>(
    entries: &'a [Entry],
    mood: &HashMap<String, f64>,
    last_pose: Option<&str>,
    rng: &mut R,
) -> Option<&'a Entry> {
    if entries.is_empty() {
        return None;
    }
    // An entry with no "pose" is excluded whenever `last_pose` is None
    // (None matches `e.get("pose")`), same as the Python `last_pose not in
    // (e.get("pose"), entry_key(e))`.
    let mut candidates: Vec<&'a Entry> = entries
        .iter()
        .filter(|entry| {
            let pose = entry.get("pose").and_then(Value::as_str);
            let key = entry_key(entry);
            last_pose != pose && last_pose != Some(key.as_str())
        })
        .collect();
    if candidates.is_empty() {
        candidates = entries.iter().collect();
    }

    let weights: Vec<f64> = candidates
        .iter()
        .map(|entry| {
            let mut weight = entry.get("weight").and_then(Value::as_f64).unwrap_or(1.0);
            if let Some(boosts) = entry.get("mood_boost").and_then(Value::as_object) {
                for (mood_key, multiplier) in boosts {
                    let multiplier = multiplier.as_f64().unwrap_or(1.0);
                    // Python: level = max(0.0, min(1.0, mood.get(mood_key, 0.0))).
                    // That equals clamp(0.0, 1.0) except for NaN: CPython's
                    // min/max drop the NaN operand (min(1.0, NaN) == 1.0),
                    // where f64::clamp would propagate NaN into the weight.
                    let raw = mood.get(mood_key.as_str()).copied().unwrap_or(0.0);
                    let level = if raw.is_nan() {
                        1.0
                    } else {
                        raw.clamp(0.0, 1.0)
                    };
                    weight *= 1.0 + (multiplier - 1.0) * level;
                }
            }
            // Python: weights.append(max(0.0, weight))
            weight.max(0.0)
        })
        .collect();

    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return Some(*rng.choice(&candidates));
    }
    let roll = rng.uniform(0.0, total);
    let mut cumulative = 0.0;
    // weights is built one-per-candidate above, so the lengths must match.
    for (entry, weight) in candidates.iter().zip(&weights) {
        cumulative += *weight;
        if roll <= cumulative {
            return Some(*entry);
        }
    }
    candidates.last().copied()
}
