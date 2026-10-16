//! faces.py (WP-12): region classification, pose loading, pose->regions
//! splitting, and emotion/tag -> pose mapping.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use felt_faces::{
    EMOTION_TO_POSE, default_face_poses_path, load_face_poses, pose_for_reaction, pose_to_regions,
    shape_region,
};

/// pytest's `tmp_path` fixture: a fresh empty temp dir per call.
fn tmp_path(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("felt-faces-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// -- shape_region -------------------------------------------------------

#[test]
fn test_brow_shape_region() {
    assert_eq!(shape_region("Fcl_BRW_Angry"), Some("brows"));
}

#[test]
fn test_eye_shape_region() {
    assert_eq!(shape_region("Fcl_EYE_Joy"), Some("eyes"));
}

#[test]
fn test_mouth_shape_region() {
    assert_eq!(shape_region("Fcl_MTH_Small"), Some("mouth"));
}

#[test]
fn test_teeth_shape_region() {
    assert_eq!(shape_region("Fcl_HA_Something"), Some("teeth"));
}

#[test]
fn test_unrecognized_shape_has_no_region() {
    assert!(shape_region("Fcl_ALL_Joy").is_none());
    assert!(shape_region("garbage").is_none());
}

// -- load_face_poses ------------------------------------------------------

#[test]
fn test_load_face_poses_real_file_has_every_wp12_pose() {
    let poses = load_face_poses(&default_face_poses_path());
    for name in [
        "soft_smile",
        "grin",
        "smug",
        "wink",
        "pout",
        "thinking",
        "sleepy",
        "wide_eyed",
        "shocked_comedic",
        "annoyed",
        "sad",
        "embarrassed",
        "curious",
        "sheepish",
    ] {
        assert!(poses.contains_key(name), "missing pose {name:?}");
    }
}

#[test]
fn test_load_face_poses_missing_file_returns_empty() {
    let dir = tmp_path("missing_file");
    assert!(load_face_poses(&dir.join("nope.json")).is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_load_face_poses_garbage_file_returns_empty() {
    let dir = tmp_path("garbage_file");
    let path = dir.join("face_poses.json");
    fs::write(&path, "not json at all").expect("write garbage file");
    assert!(load_face_poses(&path).is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_load_face_poses_real_values_round_trip() {
    let dir = tmp_path("real_values");
    let path = dir.join("face_poses.json");
    fs::write(&path, r#"{"grin": {"Fcl_EYE_Joy": 0.8}}"#).expect("write poses file");
    assert_eq!(
        load_face_poses(&path),
        HashMap::from([(
            "grin".to_string(),
            HashMap::from([("Fcl_EYE_Joy".to_string(), 0.8)])
        )])
    );
    let _ = fs::remove_dir_all(&dir);
}

// -- pose_to_regions --------------------------------------------------

#[test]
fn test_pose_to_regions_splits_by_prefix() {
    let pose = HashMap::from([
        ("Fcl_BRW_Joy".to_string(), 0.6),
        ("Fcl_EYE_Joy".to_string(), 0.8),
        ("Fcl_MTH_Fun".to_string(), 0.7),
    ]);
    assert_eq!(
        pose_to_regions(&pose),
        HashMap::from([
            (
                "brows".to_string(),
                HashMap::from([("Fcl_BRW_Joy".to_string(), 0.6)])
            ),
            (
                "eyes".to_string(),
                HashMap::from([("Fcl_EYE_Joy".to_string(), 0.8)])
            ),
            (
                "mouth".to_string(),
                HashMap::from([("Fcl_MTH_Fun".to_string(), 0.7)])
            ),
        ])
    );
}

#[test]
fn test_pose_to_regions_drops_unrecognized_shapes() {
    let pose = HashMap::from([
        ("Fcl_ALL_Joy".to_string(), 0.9),
        ("Fcl_BRW_Joy".to_string(), 0.5),
    ]);
    assert_eq!(
        pose_to_regions(&pose),
        HashMap::from([(
            "brows".to_string(),
            HashMap::from([("Fcl_BRW_Joy".to_string(), 0.5)])
        )])
    );
}

#[test]
fn test_pose_to_regions_empty_pose() {
    assert!(pose_to_regions(&HashMap::new()).is_empty());
}

// -- pose_for_reaction ------------------------------------------------

#[test]
fn test_emotion_wins_over_tag() {
    assert_eq!(
        pose_for_reaction(Some("happy"), "reaction/failure"),
        "soft_smile"
    );
}

#[test]
fn test_falls_back_to_tag_keyword_when_no_emotion() {
    assert_eq!(pose_for_reaction(None, "reaction/failure"), "sad");
}

#[test]
fn test_falls_back_to_tag_keyword_when_emotion_is_unrecognized() {
    assert_eq!(pose_for_reaction(Some("bogus"), "emotion/laughing"), "grin");
}

#[test]
fn test_no_match_falls_back_to_neutral() {
    assert_eq!(pose_for_reaction(None, "idle"), "neutral");
}

#[test]
fn test_every_emotion_value_maps_to_a_real_pose() {
    let poses = load_face_poses(&default_face_poses_path());
    for &(emotion, pose) in EMOTION_TO_POSE {
        assert!(
            pose == "neutral" || poses.contains_key(pose),
            "{emotion} -> {pose} not in face_poses.json"
        );
    }
}
