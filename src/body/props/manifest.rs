//! The props catalogue (data/props.json, from felt-animation): the items main.gd renders, which clip needs which
//! prop, the event episodes, the chest, the chair and the step sequences. Lookups on its JSON.

use serde_json::{Map, Value, json};

/// The presentation fields main.gd needs; the rest of an item entry is decisions.
const CATALOGUE_KEYS: [&str; 12] = [
    "model", "size", "fit", "rotate", "stretch", "grip", "physics", "mass", "friction", "static",
    "lid", "tint",
];

pub struct Manifest(pub Value);

impl Manifest {
    /// props.json, or an empty one (props off) if it's missing or broken.
    pub fn load(path: &std::path::Path) -> Self {
        let v: Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .filter(|v: &Value| v["items"].is_object())
            .unwrap_or_else(|| {
                tracing::warn!("props: no usable {} - props off", path.display());
                json!({ "items": {} })
            });
        Self(v)
    }

    pub fn enabled(&self) -> bool {
        self.0["items"].as_object().is_some_and(|m| !m.is_empty())
    }

    pub fn f(&self, key: &str, default: f64) -> f64 {
        self.0[key].as_f64().unwrap_or(default)
    }

    pub fn item(&self, item: &str) -> Option<&Map<String, Value>> {
        self.0["items"][item].as_object()
    }

    pub fn is_big(&self, item: &str) -> bool {
        self.item(item)
            .and_then(|e| e.get("big")?.as_bool())
            .unwrap_or(false)
    }

    pub fn is_static(&self, item: &str) -> bool {
        self.item(item)
            .and_then(|e| e.get("static")?.as_bool())
            .unwrap_or(false)
    }

    /// The floor depth an item is put down at.
    pub fn z_for(&self, item: &str) -> f64 {
        self.item(item)
            .and_then(|e| e.get("z")?.as_f64())
            .unwrap_or_else(|| self.f("floor_z", -0.25))
    }

    pub fn hand_dx(&self, seq: &str) -> f64 {
        self.0["hand_dx"][seq].as_f64().unwrap_or(0.0)
    }

    pub fn chest(&self) -> &Value {
        &self.0["chest"]
    }

    pub fn sequence(&self, name: &str) -> &[Value] {
        self.0["sequences"][name]
            .as_array()
            .map_or(&[], Vec::as_slice)
    }

    /// The prop a clip needs: "tag:clip" (one clip of a mixed bucket) wins over a whole-tag entry.
    pub fn clip_prop(&self, tag: &str, clip: Option<&str>) -> Option<Value> {
        let clips = &self.0["clips"];
        clip.and_then(|c| clips.get(format!("{tag}:{c}")))
            .or_else(|| clips.get(tag))
            .filter(|v| v.is_object())
            .cloned()
    }

    pub fn event(&self, key: &str) -> Option<Value> {
        self.0["events"].get(key).filter(|v| v.is_object()).cloned()
    }

    /// `{"prop_catalogue": {item: {model, size, grip, ...}}}`.
    pub fn catalogue_message(&self) -> Value {
        let items = self.0["items"].as_object().cloned().unwrap_or_default();
        let cat: Map<String, Value> = items
            .into_iter()
            .filter_map(|(name, e)| {
                let e = e.as_object()?;
                let kept: Map<String, Value> = e
                    .iter()
                    .filter(|(k, _)| CATALOGUE_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                Some((name, Value::Object(kept)))
            })
            .collect();
        json!({ "prop_catalogue": cat })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_catalogue_loads() {
        let m = Manifest::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/props.json"),
        );
        assert!(m.enabled());
        assert!(m.chest().is_object());
        assert_eq!(
            m.clip_prop("ambient", Some("drink_water")).unwrap()["item"],
            "water_bottle"
        );
        assert_eq!(
            m.clip_prop("ambient/snack", Some("whatever")).unwrap()["item"],
            "snack_bag"
        );
        assert!(m.clip_prop("ambient", Some("look_around")).is_none());
        assert!(!m.sequence("fetch").is_empty());
        let cat = m.catalogue_message();
        let any = cat["prop_catalogue"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        assert!(
            any.as_object()
                .unwrap()
                .keys()
                .all(|k| CATALOGUE_KEYS.contains(&k.as_str()))
        );
    }
}
