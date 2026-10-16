//! What her body knows about itself: the animation tags on this desktop (one folder per tag in the overlay's
//! library) and her face poses. Plus the fallbacks for tags a desktop may not have.

use std::collections::HashSet;
use std::path::Path;

use felt_faces::ShapeWeightsByName;
use serde_json::{Map, Value};

pub struct Assets {
    pub tags: HashSet<String>,
    /// Sorted, for felt-core's reaction pick.
    pub tag_list: Vec<String>,
    poses: ShapeWeightsByName,
}

impl Assets {
    pub fn load(animations: &Path, data_dir: &Path) -> Self {
        let tags = felt_library::scan_tags(animations);
        let mut tag_list: Vec<String> = tags.iter().cloned().collect();
        tag_list.sort();
        let poses = felt_faces::load_face_poses(&data_dir.join("face_poses.json"));
        tracing::info!(
            "her body: {} animation tags, {} face poses",
            tags.len(),
            poses.len()
        );
        Self {
            tags,
            tag_list,
            poses,
        }
    }

    /// "gesture/thinking" (a pondering pose) if this desktop has it, else plain idle.
    pub fn thinking_tag(&self) -> &str {
        ["gesture/thinking", "thinking"]
            .into_iter()
            .find(|t| self.tags.contains(*t))
            .unwrap_or("idle")
    }

    /// Her talking gesture: what plays when nothing better was picked in time.
    pub fn fallback_tag(&self) -> &str {
        if self.tags.contains("gesture/talking") {
            "gesture/talking"
        } else {
            "idle"
        }
    }

    /// A pose name -> main.gd's `{region: {shape: weight}}` (an unknown pose = a neutral face).
    pub fn pose_regions(&self, pose: &str) -> Value {
        let Some(shapes) = self.poses.get(pose) else {
            return Value::Object(Map::new());
        };
        serde_json::to_value(felt_faces::pose_to_regions(shapes)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tags(tags: &[&str]) -> Assets {
        let tags: HashSet<String> = tags.iter().map(|t| t.to_string()).collect();
        Assets {
            tag_list: tags.iter().cloned().collect(),
            tags,
            poses: ShapeWeightsByName::new(),
        }
    }

    #[test]
    fn fallbacks_only_name_tags_that_exist() {
        let a = with_tags(&["gesture/thinking", "gesture/talking"]);
        assert_eq!(a.thinking_tag(), "gesture/thinking");
        assert_eq!(a.fallback_tag(), "gesture/talking");
        let bare = with_tags(&[]);
        assert_eq!(bare.thinking_tag(), "idle");
        assert_eq!(bare.fallback_tag(), "idle");
    }

    #[test]
    fn the_real_face_poses_resolve_to_regions() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let mut a = with_tags(&[]);
        a.poses = felt_faces::load_face_poses(&data.join("face_poses.json"));
        assert!(!a.poses.is_empty());
        let (name, _) = a.poses.iter().next().unwrap();
        assert!(
            a.pose_regions(name)
                .as_object()
                .is_some_and(|o| !o.is_empty())
        );
        assert!(
            a.pose_regions("no-such-pose")
                .as_object()
                .unwrap()
                .is_empty()
        );
    }
}
