//! What every part of her body shares, on the body thread (single-threaded: `Rc`/`Cell` are fine here).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use felt_state::BehaviorState;

use super::assets::Assets;
use super::events::Events;
use super::mood::Mood;
use super::overlay::{Overlay, Rig};
use super::presence::Presence;
use super::speech::Sentence;
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
    /// Where she is (world x) as far as we know: set when she's sent somewhere.
    pub current_x: Cell<f64>,
    pub presence: RefCell<Presence>,
    /// The hide/show walk in flight (a newer one aborts it).
    pub presence_walk: RefCell<Option<tokio::task::JoinHandle<()>>>,
    /// Which edge she left through (true = screen left), so she comes back the same way.
    pub exit_left: Cell<bool>,
    /// Her sentences, in order (speech.rs).
    pub sentences: tokio::sync::mpsc::UnboundedSender<Sentence>,
    /// The Felt app's quiet switch: no spoken lines nobody asked for.
    pub quiet: Cell<bool>,
    /// Her world: the platforms, where she stands, the ledge she sits on (world.rs).
    pub world: RefCell<super::world::World>,
    /// The cursor's desktop position, as gaze last saw it.
    pub cursor: Cell<Option<(f64, f64)>>,
    pub outfits: super::outfits::Outfits,
    pub props: super::props::Props,
    /// felt-core's current weather condition, once known.
    pub weather: RefCell<Option<String>>,
    /// The overlay's animation library (one folder per tag): which clips a tag has.
    pub animations: std::path::PathBuf,
    /// Her grumbles when a window drops her (window_events.rs).
    pub grumbles: super::window_events::Grumbles,
    /// A pose holds her face until then (micro-expressions wait).
    pub face_held_until: Cell<Instant>,
    /// The "hmm" filler a slow reply gets, while armed.
    pub filler: RefCell<Option<tokio::task::JoinHandle<()>>>,
    /// Her tees folder, when configured.
    pub tees: Option<std::path::PathBuf>,
    pub wardrobe_path: std::path::PathBuf,
    /// A steady beat is playing (music.rs): she may dance.
    pub tempo_locked: Cell<bool>,
    /// A timed hush's end (the hush shortcut, "shush for 10 minutes").
    pub hush_timer: RefCell<Option<tokio::task::JoinHandle<()>>>,
    /// Her last reply's first sentence (voice + mouth cues), for "repeat that".
    pub last_reply: RefCell<Option<(Vec<u8>, Vec<serde_json::Value>)>>,
    /// Since when the quiet switch has her hushed.
    pub hushed_at: Cell<Option<Instant>>,
    /// Her recorded lines (`presence/zip_it_1.wav` ...), when configured.
    pub sounds: Option<std::path::PathBuf>,
    /// Per reaction-table key, the last pick (no immediate repeats).
    pub last_pick: RefCell<HashMap<String, String>>,
    /// The last few emotions she reacted with (her tone line's streak).
    pub streak: RefCell<Vec<String>>,
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
        self.face_held_until
            .set(Instant::now() + std::time::Duration::from_secs_f64(hold.max(0.0)));
        self.overlay
            .face(&self.assets.pose_regions(pose), hold)
            .await;
    }
}
