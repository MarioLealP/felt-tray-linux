//! Ports of `ref/felt-animation/tests/test_library.py`, plus the one pure
//! library test that lives in `test_layers.py` (posture / WP-20 upper-body).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use felt_library::{resolve_known_tag, resolve_posture_tag, scan_tags};

/// pytest's `tmp_path` fixture: a fresh empty directory per test, removed
/// when the test finishes.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(test_name: &str) -> Self {
        let dir_name = format!("felt_library_test_{}_{test_name}", std::process::id());
        let path = std::env::temp_dir().join(dir_name);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// pytest helper `_touch`: create the file (and its parents), empty.
fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, b"").expect("touch file");
}

fn tags(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

// -- TestScanTags ------------------------------------------------------------

#[test]
fn test_folder_with_only_subfolders_is_not_a_tag() {
    let tmp = TempDir::new("folder_with_only_subfolders_is_not_a_tag");
    touch(&tmp.path().join("reaction/weather/sun.vrma"));

    let tags = scan_tags(tmp.path());

    assert!(!tags.contains("reaction"));
    assert!(tags.contains("reaction/weather"));
}

#[test]
fn test_vrma_import_suffix_counts() {
    let tmp = TempDir::new("vrma_import_suffix_counts");
    touch(&tmp.path().join("idle/loop.vrma.import"));

    let tags = scan_tags(tmp.path());

    assert!(tags.contains("idle"));
}

#[test]
fn test_missing_dir_returns_empty() {
    let tmp = TempDir::new("missing_dir_returns_empty");
    assert_eq!(
        scan_tags(&tmp.path().join("does_not_exist")),
        HashSet::new()
    );
}

// -- TestResolveKnownTag -----------------------------------------------------

#[test]
fn test_exact_match() {
    let known = tags(&["reaction/weather/sun"]);
    assert_eq!(
        resolve_known_tag(Some("reaction/weather/sun"), &known),
        Some("reaction/weather/sun")
    );
}

#[test]
fn test_one_level_up() {
    let known = tags(&["reaction/weather"]);
    assert_eq!(
        resolve_known_tag(Some("reaction/weather/sun"), &known),
        Some("reaction/weather")
    );
}

#[test]
fn test_several_levels_up() {
    let known = tags(&["reaction"]);
    assert_eq!(
        resolve_known_tag(Some("reaction/weather/sun"), &known),
        Some("reaction")
    );
}

#[test]
fn test_nothing_matches() {
    let known = tags(&["idle", "emotion/happy"]);
    assert_eq!(
        resolve_known_tag(Some("reaction/weather/sun"), &known),
        None
    );
}

#[test]
fn test_none_input() {
    assert_eq!(resolve_known_tag(None, &tags(&["idle"])), None);
}

// -- TestResolvePostureTag ---------------------------------------------------

#[test]
fn test_standing_always_bare_tag() {
    let known = tags(&["idle"]);
    let (resolved, needs_stand_up) = resolve_posture_tag("idle", "standing", &known, false);
    assert_eq!(resolved, "idle");
    assert!(!needs_stand_up);
}

#[test]
fn test_sitting_with_variant() {
    let known = tags(&["idle", "idle/sitting"]);
    let (resolved, needs_stand_up) = resolve_posture_tag("idle", "sitting", &known, false);
    assert_eq!(resolved, "idle/sitting");
    assert!(!needs_stand_up);
}

#[test]
fn test_sitting_without_variant_needs_stand_up() {
    let known = tags(&["gesture/wave"]);
    let (resolved, needs_stand_up) = resolve_posture_tag("gesture/wave", "sitting", &known, false);
    assert_eq!(resolved, "gesture/wave");
    assert!(needs_stand_up);
}

// -- posture (from tests/test_layers.py) -------------------------------------

#[test]
fn test_upper_body_tag_needs_no_stand_up_while_sitting() {
    let known = tags(&[
        "emotion/happy",
        "gesture/talking",
        "gesture/talking/sitting",
        "idle/sitting",
    ]);
    assert_eq!(
        resolve_posture_tag("emotion/happy", "sitting", &known, true),
        ("emotion/happy".to_string(), false)
    );
    // A sitting variant still wins.
    assert_eq!(
        resolve_posture_tag("gesture/talking", "sitting", &known, true),
        ("gesture/talking/sitting".to_string(), false)
    );
    // Full-body tags still stand her up first.
    assert_eq!(
        resolve_posture_tag("dance", "sitting", &known, false),
        ("dance".to_string(), true)
    );
}
