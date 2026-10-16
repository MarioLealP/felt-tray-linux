//! WP-12: named face poses (`face_poses.json`) + region classification +
//! emotion/tag -> pose mapping. Replaces expressions.py's old whole-face
//! `Fcl_ALL_*` keyword table - poses here are built from REGIONAL shapes
//! (brows/eyes/mouth/teeth) so main.gd's compositor can drive each region
//! independently and keep the mouth channel free for lipsync.
//!
//! Region is inferred from each VRoid shape's own name prefix rather than
//! duplicated as metadata in `face_poses.json` - one source of truth for
//! "which shapes belong to which region", not two files that can drift.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A flat `{shape_name: weight}` mapping - one face pose, or the resting face.
pub type ShapeWeights = HashMap<String, f64>;

/// `{name: {shape_name: weight}}` - the `face_poses.json` layout (keys are
/// pose names) and [`pose_to_regions`]' result (keys are region names).
pub type ShapeWeightsByName = HashMap<String, ShapeWeights>;

/// VRoid shape-name prefix -> face region. First match wins, in the Python
/// module's `_REGION_PREFIXES` insertion order.
const REGION_PREFIXES: &[(&str, &str)] = &[
    ("Fcl_BRW_", "brows"),
    ("Fcl_EYE_", "eyes"),
    ("Fcl_MTH_", "mouth"),
    ("Fcl_HA_", "teeth"),
];

/// `DEFAULT_FACE_POSES_PATH` from the Python module (`face_poses.json` at the
/// felt-animation source-tree root). The data file is not copied into this
/// crate, so the default resolves to `data/face_poses.json` in
/// the workspace instead.
pub fn default_face_poses_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/face_poses.json")
}

/// Region for a VRoid shape name (`"Fcl_BRW_Angry"` -> `"brows"`), or [`None`]
/// when no recognized prefix matches (e.g. whole-face `Fcl_ALL_*` shapes).
pub fn shape_region(shape_name: &str) -> Option<&'static str> {
    REGION_PREFIXES
        .iter()
        .find_map(|&(prefix, region)| shape_name.starts_with(prefix).then_some(region))
}

/// Loads `face_poses.json` as `{pose_name: {shape: weight}}`. Returns an empty
/// map when the file is missing, unreadable, or not a well-formed poses object
/// (the Python original logs and no-ops - face poses are a nicety, never a
/// hard failure).
pub fn load_face_poses(path: &Path) -> ShapeWeightsByName {
    let Ok(text) = fs::read_to_string(path) else {
        // Python logs "face poses file not found ... - face poses will no-op"
        // (or an exception for unreadable/unparseable files).
        return HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Splits a flat `{shape: weight}` pose into `{region: {shape: weight}}` -
/// the `{"face": {"regions": {...}}}` relay message groups by region, not a
/// flat shape list. A shape with no recognized region prefix is dropped
/// (defensive - a typo'd shape name in face_poses.json shouldn't crash
/// anything, just silently not apply).
pub fn pose_to_regions(pose: &ShapeWeights) -> ShapeWeightsByName {
    let mut regions: ShapeWeightsByName = HashMap::new();
    for (shape, weight) in pose {
        let Some(region) = shape_region(shape) else {
            // Python logs "face pose shape %r has no recognized region prefix - dropped".
            continue;
        };
        regions
            .entry(region.to_string())
            .or_default()
            .insert(shape.clone(), *weight);
    }
    regions
}

/// Emotion -> pose name (the Python module's `EMOTION_TO_POSE` dict).
///
/// felt-core doesn't emit an emotion field for chat/skill replies yet (only
/// screen reactions do - WP-07's X-Emotion header) - mapping from the
/// resolved animation tag is the explicit fallback the WP itself offers,
/// so voice/text chat still gets a sensible pose without needing a
/// felt-core change just for this.
pub const EMOTION_TO_POSE: &[(&str, &str)] = &[
    ("happy", "soft_smile"),
    ("excited", "grin"),
    ("amused", "smug"),
    ("sad", "sad"),
    ("annoyed", "annoyed"),
    ("surprised", "wide_eyed"),
    ("thinking", "thinking"),
    ("sleepy", "sleepy"),
    ("neutral", "neutral"),
];

/// Same keyword-match spirit as the old expressions.py this replaces - a
/// newly-added animation tag gets a sensible pose automatically as long as
/// it contains a recognizable keyword, no second file to remember to
/// update every time the animation library grows.
const TAG_KEYWORD_TO_POSE: &[(&str, &str)] = &[
    ("laugh", "grin"),
    ("happy", "soft_smile"),
    ("wave", "soft_smile"),
    ("greeting", "soft_smile"),
    ("dance", "grin"),
    ("show_off", "smug"),
    ("angry", "annoyed"),
    ("embarrassed", "embarrassed"),
    ("sighing", "sad"),
    ("sad", "sad"),
    ("failure", "sad"),
    ("scared", "wide_eyed"),
    ("surprised", "wide_eyed"),
    ("stumble", "wide_eyed"),
    ("thinking", "thinking"),
    ("shrugging", "thinking"),
    ("sleepy", "sleepy"),
];

/// The emotion field (when felt-core actually sent one, e.g. a screen
/// reaction) wins; otherwise falls back to keyword-matching the resolved
/// animation tag. Always returns a real pose name - `"neutral"` if nothing
/// matches (a reply always gets SOME face, same as the old
/// expressions.py's callers expected a dict back, just now a pose name).
pub fn pose_for_reaction(emotion: Option<&str>, tag: &str) -> &'static str {
    emotion
        .and_then(|emotion| {
            EMOTION_TO_POSE
                .iter()
                .find_map(|&(known, pose)| (known == emotion).then_some(pose))
        })
        .or_else(|| {
            TAG_KEYWORD_TO_POSE
                .iter()
                .find_map(|&(keyword, pose)| tag.contains(keyword).then_some(pose))
        })
        .unwrap_or("neutral")
}

/// 0 at `start`, 1 at `end` (either direction), clamped.
fn ramp(value: f64, start: f64, end: f64) -> f64 {
    if end == start {
        return 0.0;
    }
    ((value - start) / (end - start)).clamp(0.0, 1.0)
}

/// Python's `round(weight, 2)`: the float rounded to two decimals, ties to
/// even, over its exact binary value - Rust's `{:.2}` formatting rounds the
/// same way CPython's `round` does.
fn round2(weight: f64) -> f64 {
    format!("{weight:.2}").parse().unwrap_or(weight)
}

/// WP-15 "resting face (WP-12) follows mood": the flat `{shape: weight}`
/// she shows whenever no pose holds a region - sent to main.gd as
/// `{"resting_face": {...}}`. Deliberately faint (it's on all the time):
/// content -> a faint soft_smile (~0.15 of the pose), low energy or
/// boredom -> heavier lids, annoyed -> a slight frown, unhappy -> sorrow
/// brows. Rounded so tiny drifts don't resend.
pub fn resting_face_for_mood(mood: &ShapeWeights) -> ShapeWeights {
    let energy = mood.get("energy").copied().unwrap_or(0.5);
    let happiness = mood.get("happiness").copied().unwrap_or(0.5);
    let boredom = mood.get("boredom").copied().unwrap_or(0.0);
    let annoyance = mood.get("annoyance").copied().unwrap_or(0.0);
    let mut face: ShapeWeights = HashMap::new();

    let lid = (0.3 * ramp(energy, 0.35, 0.05)).max(0.2 * ramp(boredom, 0.5, 1.0));
    if lid > 0.0 {
        face.insert("Fcl_EYE_Close".to_string(), lid);
    }
    if energy < 0.25 {
        face.insert("Fcl_BRW_Sorrow".to_string(), 0.1);
    }

    if annoyance > 0.35 {
        face.insert(
            "Fcl_BRW_Angry".to_string(),
            0.1 + 0.25 * ramp(annoyance, 0.35, 1.0),
        );
        face.insert("Fcl_MTH_Down".to_string(), 0.1);
    } else if happiness < 0.3 {
        let sorrow = face.get("Fcl_BRW_Sorrow").copied().unwrap_or(0.0).max(0.2);
        face.insert("Fcl_BRW_Sorrow".to_string(), sorrow);
        face.insert("Fcl_MTH_Down".to_string(), 0.1);
    } else if happiness >= 0.55 {
        // soft_smile (EYE_Joy .3, MTH_Up .5) at 0.15 -> 0.3 intensity
        let intensity = 0.15 + 0.15 * ramp(happiness, 0.55, 1.0);
        face.insert("Fcl_EYE_Joy".to_string(), 0.3 * intensity);
        face.insert("Fcl_MTH_Up".to_string(), 0.5 * intensity);
    }

    face.into_iter()
        .filter_map(|(shape, weight)| {
            let weight = round2(weight);
            (weight > 0.0).then_some((shape, weight))
        })
        .collect()
}
