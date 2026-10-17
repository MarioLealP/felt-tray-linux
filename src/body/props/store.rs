//! What's on the floor and what's in her hands. Placed items persist ($XDG_DATA_HOME/felt-tray/placed_props.json,
//! write-then-rename); held ones don't - a restarted overlay is empty-handed anyway.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub id: String,
    pub item: String,
    pub x: f64,
    pub z: f64,
    /// Wall clock (unix seconds): survives restarts.
    pub placed_at: f64,
}

#[derive(Default)]
pub struct Store {
    path: Option<PathBuf>,
    pub placed: BTreeMap<String, Placed>,
    /// id -> (item, bone)
    pub held: BTreeMap<String, (String, String)>,
    seq: u64,
}

pub fn now_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

impl Store {
    pub fn load(path: PathBuf) -> Self {
        let mut s = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return s;
        };
        let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
            tracing::warn!("props: {} unreadable - a clean floor", path.display());
            return s;
        };
        for row in raw["placed"].as_array().into_iter().flatten() {
            if let Ok(p) = serde_json::from_value::<Placed>(row.clone()) {
                s.placed.insert(p.id.clone(), p);
            }
        }
        s.seq = raw["seq"].as_u64().unwrap_or(0);
        for id in s.placed.keys() {
            // never hand out an id that's already on the floor
            if let Some(n) = id.rsplit('-').next().and_then(|t| t.parse::<u64>().ok()) {
                s.seq = s.seq.max(n);
            }
        }
        tracing::info!("props: {} item(s) back on the floor", s.placed.len());
        s
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let body = json!({ "seq": self.seq, "placed": self.placed.values().collect::<Vec<_>>() });
        let tmp = path.with_extension("json.tmp");
        if let Err(e) =
            std::fs::write(&tmp, body.to_string()).and_then(|()| std::fs::rename(&tmp, path))
        {
            tracing::warn!("props: can't save {}: {e}", path.display());
        }
    }

    pub fn new_id(&mut self, item: &str) -> String {
        self.seq += 1;
        format!("{item}-{}", self.seq)
    }

    pub fn add_placed(&mut self, id: &str, item: &str, x: f64, z: f64) {
        self.held.remove(id);
        let p = Placed {
            id: id.into(),
            item: item.into(),
            x: r3(x),
            z: r3(z),
            placed_at: now_s(),
        };
        self.placed.insert(id.into(), p);
        self.save();
    }

    pub fn moved(&mut self, id: &str, x: f64, z: Option<f64>) {
        if let Some(p) = self.placed.get_mut(id) {
            p.x = r3(x);
            if let Some(z) = z {
                p.z = r3(z);
            }
            self.save();
        }
    }

    pub fn hold(&mut self, id: &str, item: &str, bone: &str) {
        if self.placed.remove(id).is_some() {
            self.save();
        }
        self.held.insert(id.into(), (item.into(), bone.into()));
    }

    pub fn remove(&mut self, id: &str) {
        self.held.remove(id);
        if self.placed.remove(id).is_some() {
            self.save();
        }
    }

    pub fn held_id(&self, item: &str) -> Option<String> {
        self.held
            .iter()
            .find(|(_, (i, _))| i == item)
            .map(|(id, _)| id.clone())
    }

    pub fn placed_of(&self, item: &str) -> Vec<Placed> {
        self.placed
            .values()
            .filter(|p| p.item == item)
            .cloned()
            .collect()
    }

    /// Clean house: every id, gone.
    pub fn clear(&mut self) -> Vec<String> {
        let ids: Vec<String> = self
            .placed
            .keys()
            .chain(self.held.keys())
            .cloned()
            .collect();
        self.placed.clear();
        self.held.clear();
        self.save();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_items_survive_a_restart_and_ids_never_repeat() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("props-{}.json", std::process::id()));
        let mut s = Store::load(path.clone());
        let a = s.new_id("mug");
        s.add_placed(&a, "mug", 1.23456, -0.25);
        let b = s.new_id("book");
        s.hold(&b, "book", "RightHand");
        let again = Store::load(path.clone());
        assert_eq!(again.placed[&a].x, 1.235);
        assert!(again.held.is_empty());
        let mut again = again;
        assert_ne!(again.new_id("mug"), a);
        let _ = std::fs::remove_file(path);
    }
}
