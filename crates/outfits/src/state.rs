//! The bits of the shared runtime state this module touches.
//!
//! `outfits.py` reaches into `state`, `presence`, `wardrobe`,
//! `routers.presence` and `accessories`. Those modules are ported elsewhere;
//! here they are the minimal traits below, plus the two presence strings.

use serde_json::Value;

/// `felt_animation.presence.VISIBLE`.
pub const VISIBLE: &str = "visible";
/// `felt_animation.presence.HIDDEN`.
pub const HIDDEN: &str = "hidden";

/// The overlay relay (`{"outfit": {...}}` out, `outfit_applied` in).
/// Blocking: in Python these were awaited coroutines; the swap itself is
/// serialised by the director, so the port only needs the boolean result.
pub trait Overlay: Send + Sync {
    /// Send a relay payload; `false` when the overlay refuses it.
    fn send(&self, payload: &Value) -> bool;
    /// `wardrobe.apply_tee`'s garment swap (WP-33 tee textures).
    fn apply_garment(&self, material: &str, albedo: &str);
    /// `wardrobe.apply_tee`'s optional show-off clip.
    fn play_tag(&self, tag: &str);
}

/// An RAII stand-in for `behavior.hold_base()` (holds the idle base pose).
pub trait HoldGuard: Send {}

/// The behaviour director: busy/posture flags plus the clip-playing hooks.
pub trait Behavior: Send + Sync {
    fn busy(&self) -> bool;
    fn posture(&self) -> String;
    /// `behavior.run_sequence(tags, timeout)` - plays one short clip.
    fn run_sequence(&self, tags: Vec<String>, timeout: f64);
    /// `async with behavior.hold_base():` - returned guard is dropped on exit.
    fn hold_base(&self) -> Box<dyn HoldGuard>;
}

/// `state.presence.state` - one of the presence strings.
pub trait Presence: Send + Sync {
    fn state(&self) -> String;
}

/// `state.mood.as_dict()`'s energy field (missing -> `None`).
pub trait Mood: Send + Sync {
    fn energy(&self) -> Option<f64>;
}

/// `state.player.is_playing`.
pub trait Player: Send + Sync {
    fn is_playing(&self) -> bool;
}

/// `state.props_director.seated_on`.
pub trait PropsDirector: Send + Sync {
    fn seated_on(&self) -> Option<String>;
}

/// `state.accessories` (optional: `getattr(state, "accessories", None)`).
pub trait Accessories: Send + Sync {
    /// A restarted overlay holds nothing.
    fn reset(&self);
    /// WP-32: this outfit's accessories on, the previous outfit's off.
    fn set_outfit(&self, wanted: Vec<String>);
}

/// The presence router's walk-off / walk-back (`routers.presence`).
pub trait PresenceRouter: Send + Sync {
    /// `presence_router._do_hide()` - start the walk-off.
    fn do_hide(&self);
    /// `presence_router.do_show()` - walk back in.
    fn do_show(&self);
}

/// Everything `OutfitDirector` reads from the shared runtime state.
pub struct OutfitState {
    pub overlay: std::sync::Arc<dyn Overlay>,
    pub behavior: std::sync::Arc<dyn Behavior>,
    pub presence: std::sync::Arc<dyn Presence>,
    pub presence_router: std::sync::Arc<dyn PresenceRouter>,
    pub mood: std::sync::Arc<dyn Mood>,
    pub player: std::sync::Arc<dyn Player>,
    pub props_director: std::sync::Arc<dyn PropsDirector>,
    pub accessories: Option<std::sync::Arc<dyn Accessories>>,
    /// `state.seated_on_ledge` (which ledge id she is perched on).
    pub seated_on_ledge: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// `state.known_tags` - the animation tag vocabulary (for spin clips).
    pub known_tags: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl OutfitState {
    /// `s.seated_on_ledge is None`
    pub fn seated_on_ledge(&self) -> Option<String> {
        self.seated_on_ledge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
