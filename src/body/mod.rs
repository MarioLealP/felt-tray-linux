//! Her body on the desktop: felt-tray drives the Godot overlay that draws her (one program since 2026-10-16 -
//! the logic is the `felt-*` crates under `crates/`). It all runs on one thread of its own, single-threaded
//! like the Python event loop it was ported from; the rest of felt-tray talks to it through a [`Handle`].
//! This file only wires the parts together.

mod assets;
mod clock;
mod ctx;
mod events;
mod gaze;
mod lipsync;
mod mood;
mod overlay;
mod reply;
mod speech;

use std::cell::{Cell, RefCell};
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
use speech::Sentence;

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
pub fn start(
    cfg: &config::Body,
    core: Arc<FeltCore>,
    player: Player,
    cursor: Cursor,
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
            tokio::task::LocalSet::new().block_on(&rt, run(cfg, core, player, cursor, rx));
        })
        .context("can't start her body's thread")?;
    Ok(Handle { tx })
}

async fn run(
    cfg: config::Body,
    core: Arc<FeltCore>,
    player: Player,
    cursor: Cursor,
    mut rx: mpsc::UnboundedReceiver<Msg>,
) {
    let o = Arc::new(Overlay::new());
    tokio::spawn(overlay::supervisor::run(
        o.clone(),
        cfg.command.clone(),
        cfg.port,
    ));
    tokio::spawn(idle_on_connect(o.clone()));
    let behavior = BehaviorState::new(Rc::new(Rig(o.clone())), Rc::new(MonotonicClock::new()));
    let state_dir = crate::config::data_dir().unwrap_or_else(|_| ".".into());
    let mood_path = state_dir.join("mood.json");
    let events = events::Events::start(core.clone(), state_dir.join("day_start.json"));
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
    });
    tokio::task::spawn_local(mood::run(ctx.clone(), mood_path));
    tokio::task::spawn_local(gaze::run(ctx.clone(), cursor));
    let (sentences, queue) = mpsc::unbounded_channel();
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
            } => {
                let generation = ctx.next_generation();
                ctx.bubble.replace(text.clone());
                let reaction = Some(reply::begin(&ctx, &text, tag, emotion, prompted));
                let _ = sentences.send(Sentence {
                    generation,
                    text,
                    audio,
                    reaction,
                });
            }
            Msg::More { text, audio } => {
                let generation = ctx.generation.get();
                let _ = sentences.send(Sentence {
                    generation,
                    text,
                    audio,
                    reaction: None,
                });
            }
        }
    }
}

/// A (re)started overlay starts from nothing: idle.
async fn idle_on_connect(o: Arc<Overlay>) {
    let mut rx = o.subscribe();
    loop {
        match rx.recv().await {
            Ok(Event::Connected) => {
                o.play_tag("idle", false, None).await;
            }
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(_) => return,
        }
    }
}
