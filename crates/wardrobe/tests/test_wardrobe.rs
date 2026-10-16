//! Port of `tests/test_wardrobe.py` (WP-21/WP-27): the daily-tee roll,
//! its 21-day cooldown, and the state-file fallback. WP-40 item 19
//! (WP-38 audit): the Python module was the only source module in the
//! repo with zero test coverage.
//!
//! Every Python test monkeypatches the module's own
//! `WARDROBE_JSON_PATH`/`TEES_APPROVED_PATH`/`TEES_DIR` (now
//! config-driven, WP-40 item 18/2) to `tmp_path` locations — no real
//! file under `E:\Felt` is ever touched. Here the same paths are passed
//! in via `WardrobeConfig`, and the clock/`random.choice` the Python
//! module calls directly are injected as the `today`/`choose_index`
//! arguments (a fixed date and a fixed pick, so the tests are
//! deterministic).

use std::fs;
use std::path::{Path, PathBuf};

use felt_wardrobe::{
    Date, Days, Error, Overlay, WardrobeConfig, apply_tee, check_and_roll_daily,
    load_approved_tees, load_wardrobe_state, roll_tee,
};
use serde_json::{Value, json};

/// The Python tests roll against `datetime.date.today()`; the clock is
/// injected, so a fixed date stands in.
fn today() -> Date {
    Date::from_ymd(2026, 10, 13).unwrap()
}

/// Deterministic stand-in for `random.choice`: always the first
/// candidate. The Python tests never assert a particular random pick
/// (single-candidate rolls or membership checks), so a fixed index keeps
/// them equivalent.
fn first_choice(_len: usize) -> usize {
    0
}

/// A disposable directory under the system temp dir (pytest's
/// `tmp_path`), removed again when the test finishes.
struct TmpPath {
    path: PathBuf,
}

impl TmpPath {
    fn new(test_name: &str) -> TmpPath {
        let path =
            std::env::temp_dir().join(format!("felt-wardrobe-{}-{test_name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create tmp dir");
        TmpPath { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpPath {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Mirrors `_setup(monkeypatch, tmp_path, approved)`: points the module
/// at `tmp_path` files and optionally writes the curated approved list.
fn setup(tmp_path: &Path, approved: Option<&[&str]>) -> WardrobeConfig {
    let config = WardrobeConfig {
        wardrobe_json_path: tmp_path.join("wardrobe.json"),
        tees_approved_path: tmp_path.join("tees_approved.json"),
        tees_dir: format!("{}/tees", tmp_path.display()),
    };
    if let Some(approved) = approved {
        fs::write(
            &config.tees_approved_path,
            serde_json::to_string(&approved).unwrap(),
        )
        .expect("write approved tees");
    }
    config
}

/// Python's `wardrobe_path.write_text(json.dumps(...))`.
fn write_state(config: &WardrobeConfig, state: Value) {
    fs::write(
        &config.wardrobe_json_path,
        serde_json::to_string(&state).unwrap(),
    )
    .expect("write wardrobe state");
}

// -- load_wardrobe_state fallback -------------------------------------------

#[test]
fn test_load_wardrobe_state_defaults_when_file_is_missing() {
    let tmp = TmpPath::new("state_defaults_when_file_is_missing");
    let config = setup(tmp.path(), None);
    assert_eq!(
        load_wardrobe_state(&config).unwrap(),
        json!({"current_tee": null, "last_worn_date": null, "history": {}})
    );
}

#[test]
fn test_load_wardrobe_state_reads_an_existing_file() {
    let tmp = TmpPath::new("state_reads_an_existing_file");
    let config = setup(tmp.path(), None);
    write_state(
        &config,
        json!({"current_tee": "cat_shirt", "last_worn_date": "2026-09-01", "history": {}}),
    );
    let state = load_wardrobe_state(&config).unwrap();
    assert_eq!(state["current_tee"], json!("cat_shirt"));
}

// -- load_approved_tees ------------------------------------------------------

#[test]
fn test_load_approved_tees_missing_file_returns_empty() {
    let tmp = TmpPath::new("approved_missing_file_returns_empty");
    let config = setup(tmp.path(), None);
    assert_eq!(load_approved_tees(&config).unwrap(), Vec::<String>::new());
}

#[test]
fn test_load_approved_tees_reads_the_curated_list() {
    let tmp = TmpPath::new("approved_reads_the_curated_list");
    let config = setup(tmp.path(), Some(&["cat_shirt", "dog_shirt"]));
    assert_eq!(
        load_approved_tees(&config).unwrap(),
        vec!["cat_shirt".to_owned(), "dog_shirt".to_owned()]
    );
}

// -- roll_tee's 21-day cooldown -----------------------------------------------

#[test]
fn test_roll_tee_with_no_approved_tees_returns_none() {
    let tmp = TmpPath::new("roll_with_no_approved_tees");
    let config = setup(tmp.path(), Some(&[]));
    assert_eq!(roll_tee(&config, today(), &mut first_choice).unwrap(), None);
}

#[test]
fn test_roll_tee_excludes_tees_worn_within_21_days() {
    let tmp = TmpPath::new("roll_excludes_within_21_days");
    let config = setup(tmp.path(), Some(&["recent_shirt", "old_shirt"]));
    let today = today();
    let recently_worn = (today - Days::new(5)).isoformat();
    write_state(
        &config,
        json!({
            "current_tee": "recent_shirt",
            "last_worn_date": recently_worn.clone(),
            "history": {"recent_shirt": recently_worn},
        }),
    );

    let chosen = roll_tee(&config, today, &mut first_choice).unwrap();

    assert_eq!(chosen.as_deref(), Some("old_shirt")); // the only one not on cooldown
}

#[test]
fn test_roll_tee_a_tee_worn_exactly_21_days_ago_is_valid_again() {
    let tmp = TmpPath::new("roll_21_days_ago_is_valid_again");
    let config = setup(tmp.path(), Some(&["only_shirt"]));
    let today = today();
    let worn_21_days_ago = (today - Days::new(21)).isoformat();
    write_state(
        &config,
        json!({"history": {"only_shirt": worn_21_days_ago}}),
    );

    assert_eq!(
        roll_tee(&config, today, &mut first_choice)
            .unwrap()
            .as_deref(),
        Some("only_shirt")
    );
}

#[test]
fn test_roll_tee_ignores_the_cooldown_when_everything_is_on_cooldown() {
    let tmp = TmpPath::new("roll_ignores_cooldown_when_all_on_cooldown");
    let config = setup(tmp.path(), Some(&["a_shirt", "b_shirt"]));
    let today = today();
    let yesterday = (today - Days::new(1)).isoformat();
    write_state(
        &config,
        json!({"history": {"a_shirt": yesterday, "b_shirt": yesterday}}),
    );

    // Every approved tee is on cooldown - still rolls one rather than going
    // shirtless (roll_tee's own documented fallback).
    let chosen = roll_tee(&config, today, &mut first_choice).unwrap();
    assert!(matches!(
        chosen.as_deref(),
        Some("a_shirt") | Some("b_shirt")
    ));
}

#[test]
fn test_roll_tee_persists_the_pick_and_history() {
    let tmp = TmpPath::new("roll_persists_the_pick_and_history");
    let config = setup(tmp.path(), Some(&["only_shirt"]));
    let today = today();

    let chosen = roll_tee(&config, today, &mut first_choice).unwrap();

    assert_eq!(chosen.as_deref(), Some("only_shirt"));
    let saved: Value =
        serde_json::from_str(&fs::read_to_string(&config.wardrobe_json_path).unwrap()).unwrap();
    assert_eq!(saved["current_tee"], json!("only_shirt"));
    assert_eq!(saved["last_worn_date"], json!(today.isoformat()));
    assert_eq!(saved["history"]["only_shirt"], json!(today.isoformat()));
}

// -- check_and_roll_daily -----------------------------------------------------

#[test]
fn test_check_and_roll_daily_rolls_on_a_new_day() {
    let tmp = TmpPath::new("check_rolls_on_a_new_day");
    let config = setup(tmp.path(), Some(&["only_shirt"]));
    write_state(
        &config,
        json!({"current_tee": "only_shirt", "last_worn_date": "2020-01-01", "history": {}}),
    );

    let (rolled, tee) = check_and_roll_daily(&config, today(), &mut first_choice).unwrap();

    assert!(rolled);
    assert_eq!(tee.as_deref(), Some("only_shirt"));
}

#[test]
fn test_check_and_roll_daily_keeps_todays_tee() {
    let tmp = TmpPath::new("check_keeps_todays_tee");
    let config = setup(tmp.path(), Some(&[])); // roll_tee would return None
    let today = today();
    let today_str = today.isoformat();
    write_state(
        &config,
        json!({"current_tee": "already_worn_today", "last_worn_date": today_str}),
    );

    let (rolled, tee) = check_and_roll_daily(&config, today, &mut first_choice).unwrap();

    assert!(!rolled);
    assert_eq!(tee.as_deref(), Some("already_worn_today")); // not re-rolled to None
}

// -- apply_tee -----------------------------------------------------------------

/// `_FakeOverlay` from the Python tests: records the calls.
#[derive(Debug, PartialEq, Eq)]
struct Garment {
    material: String,
    albedo: String,
}

#[derive(Default)]
struct FakeOverlay {
    played_tags: Vec<String>,
    garments: Vec<Garment>,
}

impl Overlay for FakeOverlay {
    fn play_tag(&mut self, tag: &str) -> Result<(), Error> {
        self.played_tags.push(tag.to_owned());
        Ok(())
    }

    fn apply_garment(&mut self, material: &str, albedo: &str) -> Result<(), Error> {
        self.garments.push(Garment {
            material: material.to_owned(),
            albedo: albedo.to_owned(),
        });
        Ok(())
    }
}

#[test]
fn test_apply_tee_builds_the_albedo_path_from_tees_dir() {
    let tmp = TmpPath::new("apply_tee_builds_the_albedo_path");
    let config = setup(tmp.path(), None);
    let mut overlay = FakeOverlay::default();

    apply_tee(&mut overlay, &config, "cat_shirt", false).unwrap();

    assert_eq!(
        overlay.garments,
        vec![Garment {
            material: "N00_004_01_Tops_01_CLOTH".to_owned(),
            albedo: format!("{}/cat_shirt.png", config.tees_dir),
        }]
    );
    assert_eq!(overlay.played_tags, Vec::<String>::new()); // visible=False (default) - no show-off gesture
}

#[test]
fn test_apply_tee_visible_plays_the_show_off_gesture_first() {
    let tmp = TmpPath::new("apply_tee_visible_plays_show_off");
    let config = setup(tmp.path(), None);
    let mut overlay = FakeOverlay::default();

    apply_tee(&mut overlay, &config, "dog_shirt", true).unwrap();

    assert_eq!(overlay.played_tags, vec!["ambient/show_off".to_owned()]);
    assert_eq!(
        overlay.garments[0].albedo,
        format!("{}/dog_shirt.png", config.tees_dir)
    );
}
