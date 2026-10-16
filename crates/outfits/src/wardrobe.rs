//! The wardrobe.json state store (shared with the WP-33 tee fields).
//!
//! Shim of the bits of `felt_animation.wardrobe` this module drives:
//! load/save of the flat state dict, and `apply_tee`. The path is injectable
//! (Python monkeypatches `WARDROBE_JSON_PATH` in the tests).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::state::Overlay;

/// `felt_animation.wardrobe` as the outfit director uses it.
pub trait WardrobeStore: Send + Sync {
    /// `load_wardrobe_state()` - missing file -> the empty tee state.
    fn load(&self) -> BTreeMap<String, Value>;
    /// `save_wardrobe_state(state)` - round-trips other fields untouched.
    fn save(&self, state: &BTreeMap<String, Value>);
    /// `apply_tee(overlay, tee_name)` - re-shirt her after a default swap.
    fn apply_tee(&self, overlay: &dyn Overlay, tee_name: &str);
}

/// File-backed store for one wardrobe.json.
pub struct FileWardrobe {
    path: PathBuf,
    tees_dir: String,
}

impl FileWardrobe {
    /// `tees_dir` is the WP-33 texture directory (`config.tees_dir`).
    pub fn new(path: impl Into<PathBuf>, tees_dir: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            tees_dir: tees_dir.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl WardrobeStore for FileWardrobe {
    fn load(&self) -> BTreeMap<String, Value> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(_) => {
                let mut empty = BTreeMap::new();
                empty.insert("current_tee".to_string(), Value::Null);
                empty.insert("last_worn_date".to_string(), Value::Null);
                empty.insert("history".to_string(), Value::Object(Default::default()));
                return empty;
            }
        };
        // Python lets a corrupt file raise; panic is the same failure mode.
        serde_json::from_str(&text).expect("wardrobe: invalid wardrobe.json")
    }

    fn save(&self, state: &BTreeMap<String, Value>) {
        let text = serde_json::to_string_pretty(state).expect("wardrobe: state serialises");
        std::fs::write(&self.path, text).expect("wardrobe: cannot write wardrobe.json");
    }

    fn apply_tee(&self, overlay: &dyn Overlay, tee_name: &str) {
        let albedo = format!("{}/{tee_name}.png", self.tees_dir);
        overlay.apply_garment("N00_004_01_Tops_01_CLOTH", &albedo);
    }
}
