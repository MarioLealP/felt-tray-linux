//! WP-21/WP-27: daily tee rotation. Each day she rolls one of Mario's
//! approved graphic tees ([`roll_tee`]'s own 21-day-cooldown-per-tee rule,
//! falling back to "all worn recently, ignore the cooldown" rather than
//! refusing to dress her), applies it as the overlay's shirt
//! garment/albedo texture ([`apply_tee`]), and persists which one's
//! currently worn plus a per-tee last-worn history so the cooldown
//! survives a restart.
//!
//! WP-40 item 18 (WP-38 audit): the Python module used to hardcode three
//! absolute `E:\Felt\...` paths — the one module in the repo breaking the
//! "no hardcoded paths" rule. They are `config.py` fields now
//! (`FELT_WARDROBE_STATE_PATH`/`FELT_TEES_APPROVED_PATH`/`FELT_TEES_DIR`),
//! passed in here as [`WardrobeConfig`].
//!
//! The Python module also reads `datetime.date.today()` and calls
//! `random.choice` directly. Both are injected here (the `today`
//! parameter and the `choose_index` callback) so callers and tests stay
//! deterministic; [`Date`]/[`Days`] cover the `datetime.date` surface the
//! cooldown logic needs.

mod date;

pub use date::{Date, DateError, Days};

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Map, Value, json};

/// The shirt garment's material slot on the overlay model — the target of
/// [`apply_tee`]'s texture swap.
pub const SHIRT_MATERIAL: &str = "N00_004_01_Tops_01_CLOTH";

/// Gesture tag played before showing off a freshly rolled tee.
pub const SHOW_OFF_TAG: &str = "ambient/show_off";

/// How many days a tee stays on cooldown after being worn
/// ([`roll_tee`]; a tee worn exactly this many days ago is valid again).
pub const COOLDOWN_DAYS: i64 = 21;

/// The files this module reads and writes — the three `config.py` fields
/// that replaced the Python module's hardcoded paths
/// (`WARDROBE_JSON_PATH`/`TEES_APPROVED_PATH`/`TEES_DIR`). The Python
/// module keeps them as module-level globals that its tests monkeypatch;
/// here every entry point takes them explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WardrobeConfig {
    /// `FELT_WARDROBE_STATE_PATH` — the persisted state JSON (current tee
    /// plus per-tee history; other modules also keep fields in it).
    pub wardrobe_json_path: PathBuf,
    /// `FELT_TEES_APPROVED_PATH` — curated JSON list of approved tee names.
    pub tees_approved_path: PathBuf,
    /// `FELT_TEES_DIR` — directory holding the `{tee}.png` albedo textures.
    /// Kept as a plain string because Python builds the overlay path with
    /// `f"{TEES_DIR}/{tee_name}.png"` string formatting.
    pub tees_dir: String,
}

/// Everything that can go wrong reading/writing the state files or
/// talking to the overlay. The Python module lets the corresponding
/// exceptions (`OSError`, `ValueError`, `TypeError`,
/// `json.JSONDecodeError`) propagate to the caller.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(transparent)]
    Date(#[from] DateError),
    /// The state file didn't contain a JSON object (the Python `dict`).
    #[error("wardrobe state must be a JSON object")]
    StateNotAnObject,
    /// `history` was present but wasn't a JSON object.
    #[error("\"history\" in the wardrobe state must be a JSON object")]
    HistoryNotAnObject,
    /// A truthy `history` entry wasn't a date string.
    #[error("last-worn entry for {tee:?} is not a date")]
    HistoryEntryNotADate { tee: String },
    /// Error reported by an [`Overlay`] implementation.
    #[error("overlay error: {0}")]
    Overlay(String),
}

/// The overlay's garment/animation surface. The Python `overlay` object
/// is an async client to the overlay (Godot) process; it is modelled as a
/// synchronous trait here (this crate is pure logic — no async runtime)
/// and deliberately has no real implementation.
pub trait Overlay {
    /// `await overlay.play_tag(tag)` — queue an animation tag.
    fn play_tag(&mut self, tag: &str) -> Result<(), Error>;

    /// `await overlay.apply_garment(material = ..., albedo = ...)` —
    /// swap a garment's albedo texture.
    fn apply_garment(&mut self, material: &str, albedo: &str) -> Result<(), Error>;
}

/// `load_approved_tees` — the curated tee list, or `[]` when the file
/// hasn't been created yet (Python logs a warning and returns `[]`).
pub fn load_approved_tees(config: &WardrobeConfig) -> Result<Vec<String>, Error> {
    if !config.tees_approved_path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&config.tees_approved_path).map_err(|source| Error::Read {
        path: config.tees_approved_path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| Error::Json {
        path: config.tees_approved_path.clone(),
        source,
    })
}

/// `load_wardrobe_state` — the persisted state dict, or the default
/// `{"current_tee": null, "last_worn_date": null, "history": {}}` when
/// the file doesn't exist yet. The result is always a JSON object (the
/// Python `dict`); unknown keys (e.g. the outfits module's `outfit_*`
/// fields) are preserved untouched.
pub fn load_wardrobe_state(config: &WardrobeConfig) -> Result<Value, Error> {
    if !config.wardrobe_json_path.exists() {
        return Ok(json!({"current_tee": null, "last_worn_date": null, "history": {}}));
    }
    let text = fs::read_to_string(&config.wardrobe_json_path).map_err(|source| Error::Read {
        path: config.wardrobe_json_path.clone(),
        source,
    })?;
    let state: Map<String, Value> = serde_json::from_str(&text).map_err(|source| Error::Json {
        path: config.wardrobe_json_path.clone(),
        source,
    })?;
    Ok(Value::Object(state))
}

/// `save_wardrobe_state` — write the state dict back as indented JSON
/// (Python: `json.dump(state, f, indent = 4)`).
pub fn save_wardrobe_state(config: &WardrobeConfig, state: &Value) -> Result<(), Error> {
    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut buf,
        serde_json::ser::PrettyFormatter::with_indent(b"    "),
    );
    state
        .serialize(&mut serializer)
        .map_err(|source| Error::Json {
            path: config.wardrobe_json_path.clone(),
            source,
        })?;
    fs::write(&config.wardrobe_json_path, buf).map_err(|source| Error::Write {
        path: config.wardrobe_json_path.clone(),
        source,
    })
}

/// `check_and_roll_daily` — roll a fresh tee when the state's
/// `last_worn_date` isn't today yet, otherwise keep today's pick.
/// Returns `(rolled, tee)`.
///
/// `choose_index` is the injected `random.choice` (see [`roll_tee`]).
pub fn check_and_roll_daily(
    config: &WardrobeConfig,
    today: Date,
    choose_index: &mut dyn FnMut(usize) -> usize,
) -> Result<(bool, Option<String>), Error> {
    let state = load_wardrobe_state(config)?;
    let today_str = today.isoformat();
    if state.get("last_worn_date").and_then(Value::as_str) != Some(today_str.as_str()) {
        return Ok((true, roll_tee(config, today, choose_index)?));
    }
    Ok((
        false,
        state
            .get("current_tee")
            .and_then(Value::as_str)
            .map(|s| s.to_owned()),
    ))
}

/// `roll_tee` — pick today's tee from the approved list, skipping tees
/// worn within the last [`COOLDOWN_DAYS`] days (falling back to the whole
/// approved list when everything is on cooldown rather than refusing to
/// dress her), then persist the pick and the per-tee history.
///
/// `choose_index` is the injected `random.choice`: given the number of
/// candidates it returns the index to take (must be `< len`). Returns
/// `None` when no tees are approved.
pub fn roll_tee(
    config: &WardrobeConfig,
    today: Date,
    choose_index: &mut dyn FnMut(usize) -> usize,
) -> Result<Option<String>, Error> {
    let approved = load_approved_tees(config)?;
    if approved.is_empty() {
        return Ok(None);
    }

    let mut state = load_wardrobe_state(config)?;
    // Python: `history = state.get("history", {})` — a missing key counts
    // as empty; anything that isn't an object is an error.
    let empty_history = Map::new();
    let history = match state.get("history") {
        None => &empty_history,
        Some(Value::Object(h)) => h,
        Some(_) => return Err(Error::HistoryNotAnObject),
    };

    let today_str = today.isoformat();
    let mut valid_tees = Vec::new();
    for t in &approved {
        // Python: `if last_worn_str:` — falsy values (null, "", 0,
        // false, [], {}) count as "never worn".
        let last_worn = match history.get(t) {
            Some(v) if py_truthy(v) => match v {
                Value::String(s) => Some(Date::from_isoformat(s)?),
                _ => return Err(Error::HistoryEntryNotADate { tee: t.clone() }),
            },
            _ => None,
        };
        if last_worn.is_some_and(|last_worn| (today - last_worn).days < COOLDOWN_DAYS) {
            continue;
        }
        valid_tees.push(t.clone());
    }

    if valid_tees.is_empty() {
        // All approved tees have been worn in the last 21 days — ignore
        // the cooldown instead of going shirtless.
        valid_tees = approved;
    }

    let chosen = valid_tees[choose_index(valid_tees.len())].clone();

    let state_obj = state.as_object_mut().ok_or(Error::StateNotAnObject)?;
    state_obj.insert("current_tee".to_owned(), Value::String(chosen.clone()));
    state_obj.insert(
        "last_worn_date".to_owned(),
        Value::String(today_str.clone()),
    );
    // Python: `if "history" not in state: state["history"] = {}`, then
    // `state["history"][chosen] = today_str`.
    let history_obj = state_obj
        .entry("history".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let history_obj = history_obj
        .as_object_mut()
        .ok_or(Error::HistoryNotAnObject)?;
    history_obj.insert(chosen.clone(), Value::String(today_str));

    save_wardrobe_state(config, &state)?;
    Ok(Some(chosen))
}

/// `apply_tee` — show the tee as the overlay's shirt texture (an albedo
/// under `tees_dir`), playing the show-off gesture first when
/// `visible` is set (the fresh-roll case).
pub fn apply_tee(
    overlay: &mut impl Overlay,
    config: &WardrobeConfig,
    tee_name: &str,
    visible: bool,
) -> Result<(), Error> {
    let tee_path = format!("{}/{tee_name}.png", config.tees_dir);
    if visible {
        overlay.play_tag(SHOW_OFF_TAG)?;
    }
    overlay.apply_garment(SHIRT_MATERIAL, &tee_path)
}

/// Python truthiness for a JSON value — the module's `if last_worn_str:`
/// treats `None`/`""`/`0`/`false`/`[]`/`{}` as "never worn".
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_none_or(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
