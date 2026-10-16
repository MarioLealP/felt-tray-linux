//! What every part of her body shares, on the body thread (single-threaded: `Rc`/`Cell` are fine here).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use felt_state::BehaviorState;

use super::assets::Assets;
use super::events::Events;
use super::mood::Mood;
use super::overlay::{Overlay, Rig};
use crate::audio::Player;
use crate::core::FeltCore;

/// felt-animation's `CLIP_TIMEOUT`: the safety net on any one clip's `finished` notice.
pub const CLIP_TIMEOUT_S: f64 = 60.0;
/// A reply's speech bubble stays at least this long.
pub const BUBBLE_MIN_S: f64 = 6.0;

pub struct Ctx {
    pub overlay: Arc<Overlay>,
    pub behavior: Rc<BehaviorState<Rig>>,
    pub assets: Assets,
    pub core: Arc<FeltCore>,
    pub player: Player,
    pub rhubarb: Vec<String>,
    /// Bumped by every new reply (and thinking): queued speech from an older one is dropped.
    pub generation: Cell<u64>,
    /// The current reply's face pose and bubble text, kept up while its later sentences play.
    pub pose: RefCell<String>,
    pub bubble: RefCell<String>,
    /// Between a line going out and her reply starting (her gaze glances up-left).
    pub thinking: Cell<bool>,
    /// She's talking until then (her gaze is on you); a little past each sentence so the gaps between
    /// streamed sentences don't count as silence.
    pub speaking_until: Cell<Instant>,
    pub mood: RefCell<Mood>,
    pub events: Events,
}

impl Ctx {
    pub fn next_generation(&self) -> u64 {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        g
    }

    pub fn current(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }

    /// A face pose held `hold` seconds.
    pub async fn apply_pose(&self, pose: &str, hold: f64) {
        self.overlay
            .face(&self.assets.pose_regions(pose), hold)
            .await;
    }
}
