//! Her body on the desktop: felt-tray drives the Godot overlay that draws her (one program since 2026-10-16 -
//! the logic is the `felt-*` crates under `crates/`). It all runs on one thread of its own, single-threaded
//! like the Python event loop it was ported from; the rest of felt-tray talks to it through a [`Handle`].
//! This file only wires the parts together.

mod ambient;
mod assets;
mod clock;
mod ctx;
mod events;
mod gaze;
mod hush;
mod ledge;
mod lipsync;
mod mood;
mod moves;
mod music;
mod nav;
mod outfits;
mod overlay;
mod presence;
mod react;
mod reply;
mod signals;
mod speech;
mod tees;
mod world;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use felt_state::{BehaviorState, MonotonicClock};
use tokio::sync::{mpsc, watch};

use crate::audio::Player;
use crate::config;
use crate::core::FeltCore;
use assets::Assets;
use ctx::{CLIP_TIMEOUT_S, Ctx};
use overlay::{Event, Overlay, Rig};

enum Msg {
    /// A line went to felt-core: she thinks until the reply comes.
    Thinking,
    /// A reply's first sentence.
    Reply {
        text: String,
        audio: Vec<u8>,
        tag: Option<String>,
        emotion: Option<String>,
        prompted: bool,
    },
    /// A later sentence of the same reply (streamed).
    More { text: String, audio: Vec<u8> },
    /// The Felt app's hide switch.
    Hidden(bool),
    /// The Felt app's quiet switch.
    Quiet(bool),
    /// Onto the ledge nearest the cursor, and sit.
    GoToLedge,
}

/// felt-tray's side of her body. Cheap to clone; every call returns at once.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Msg>,
}

impl Handle {
    pub fn thinking(&self) {
        let _ = self.tx.send(Msg::Thinking);
    }

    /// She says `text` (the WAV) with a reaction; `tag` is felt-core's animation tag, `emotion` its emotion.
    pub fn reply(
        &self,
        text: &str,
        audio: Vec<u8>,
        tag: Option<String>,
        emotion: Option<String>,
        prompted: bool,
    ) {
        let text = text.to_owned();
        let _ = self.tx.send(Msg::Reply {
            text,
            audio,
            tag,
            emotion,
            prompted,
        });
    }

    /// The hide switch: she walks off screen (true) or comes back (false).
    pub fn set_hidden(&self, hidden: bool) {
        let _ = self.tx.send(Msg::Hidden(hidden));
    }

    pub fn go_to_ledge(&self) {
        let _ = self.tx.send(Msg::GoToLedge);
    }

    pub fn set_quiet(&self, quiet: bool) {
        let _ = self.tx.send(Msg::Quiet(quiet));
    }

    pub fn more(&self, text: &str, audio: Vec<u8>) {
        let text = text.to_owned();
        let _ = self.tx.send(Msg::More { text, audio });
    }
}

/// The cursor's desktop position, from our KWin cursor script.
pub type Cursor = watch::Receiver<Option<(f64, f64)>>;

const CURSOR_PLUGIN: &str = "felt-tray-cursor";

/// KDE: our cursor script, so her eyes can follow the mouse.
pub async fn load_cursor_script(conn: &zbus::Connection) -> Result<()> {
    crate::screen::kwin::load(conn, CURSOR_PLUGIN, include_str!("cursor_script.js")).await
}

pub async fn unload_cursor_script(conn: &zbus::Connection) -> Result<()> {
    crate::screen::kwin::unload(conn, CURSOR_PLUGIN).await
}

/// Starts her body's thread: the overlay, its link, and her reactions to what felt-tray tells it.
/// KWin's window list (games, later her world).
pub type Windows = watch::Receiver<crate::screen::kwin::Snapshot>;

pub fn start(
    cfg: &config::Body,
    core: Arc<FeltCore>,
    player: Player,
    cursor: Cursor,
    windows: Windows,
) -> Result<Handle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let cfg = cfg.clone();
    std::thread::Builder::new()
        .name("felt-body".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("her body's runtime");
            let body = run(cfg, core, player, cursor, windows, rx);
            tokio::task::LocalSet::new().block_on(&rt, body);
        })
        .context("can't start her body's thread")?;
    Ok(Handle { tx })
}

async fn run(
    cfg: config::Body,
    core: Arc<FeltCore>,
    player: Player,
    cursor: Cursor,
    windows: Windows,
    mut rx: mpsc::UnboundedReceiver<Msg>,
) {
    let o = Arc::new(Overlay::new());
    tokio::spawn(overlay::supervisor::run(
        o.clone(),
        cfg.command.clone(),
        cfg.port,
    ));
    let behavior = BehaviorState::new(Rc::new(Rig(o.clone())), Rc::new(MonotonicClock::new()));
    let state_dir = crate::config::data_dir().unwrap_or_else(|_| ".".into());
    let mood_path = state_dir.join("mood.json");
    let wardrobe_path = outfits::wardrobe_path(&state_dir);
    let outfits = outfits::Outfits::load(&cfg.data_dir, &wardrobe_path, cfg.outfit_ambient_chance);
    let events = events::Events::start(core.clone(), state_dir.join("day_start.json"));
    let (sentences, queue) = mpsc::unbounded_channel();
    let ctx = Rc::new(Ctx {
        overlay: o,
        behavior: Rc::new(behavior),
        assets: Assets::load(&cfg.animations, &cfg.data_dir),
        core,
        player,
        rhubarb: cfg.rhubarb.clone(),
        generation: Cell::new(0),
        pose: RefCell::new("neutral".into()),
        bubble: RefCell::new(String::new()),
        thinking: Cell::new(false),
        speaking_until: Cell::new(Instant::now()),
        mood: RefCell::new(mood::load(&mood_path)),
        events,
        current_x: Cell::new(0.0),
        presence: RefCell::new(presence::Presence::new(clock::monotonic_s as fn() -> f64)),
        presence_walk: RefCell::new(None),
        exit_left: Cell::new(true),
        sentences,
        quiet: Cell::new(false),
        sounds: cfg.sounds.clone(),
        hushed_at: Cell::new(None),
        tempo_locked: Cell::new(false),
        world: RefCell::new(world::World::default()),
        outfits,
        tees: cfg.tees.clone(),
        wardrobe_path,
        cursor: Cell::new(None),
        last_pick: RefCell::new(HashMap::new()),
        streak: RefCell::new(Vec::new()),
    });
    tokio::task::spawn_local(overlay_events(ctx.clone()));
    tokio::task::spawn_local(mood::run(ctx.clone(), mood_path));
    tokio::task::spawn_local(ambient::idle_refresh(ctx.clone()));
    tokio::task::spawn_local(ambient::run(ctx.clone()));
    tokio::task::spawn_local(gaze::run(ctx.clone(), cursor));
    tokio::task::spawn_local(world::run(ctx.clone(), windows.clone()));
    tokio::task::spawn_local(signals::run(ctx.clone(), windows, cfg.data_dir.clone()));
    if !cfg.music_capture.is_empty() {
        tokio::task::spawn_local(music::run(ctx.clone(), cfg.music_capture.clone()));
    }
    tokio::task::spawn_local(speech::run(ctx.clone(), queue));
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Thinking => {
                ctx.next_generation();
                ctx.thinking.set(true);
                ctx.overlay.lipsync(&[]).await; // a cut-off reply's mouth eases closed
                ctx.behavior
                    .begin_thinking(ctx.assets.thinking_tag(), CLIP_TIMEOUT_S);
            }
            Msg::Reply {
                text,
                audio,
                tag,
                emotion,
                prompted,
            } => speech::say(&ctx, text, audio, tag, emotion, prompted),
            Msg::Hidden(true) => presence::hide(&ctx),
            Msg::Hidden(false) => presence::show(&ctx),
            Msg::Quiet(q) => hush::set(&ctx, q).await,
            Msg::GoToLedge => {
                let c = ctx.clone();
                tokio::task::spawn_local(async move { nav::go_to_ledge(&c).await });
            }
            Msg::More { text, audio } => speech::more(&ctx, text, audio),
        }
    }
}

/// A (re)started overlay starts from nothing: idle, and her outfit back on; main.gd's outfit answers.
async fn overlay_events(ctx: Rc<Ctx>) {
    let mut rx = ctx.overlay.subscribe();
    loop {
        match rx.recv().await {
            Ok(Event::Connected) => {
                ctx.overlay.play_tag("idle", false, None).await;
                outfits::on_connected(&ctx).await;
            }
            Ok(Event::Message(m)) => outfits::on_message(&ctx, &m).await,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(_) => return,
        }
    }
}
