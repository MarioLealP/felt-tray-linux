//! Her body on the desktop: felt-tray drives the Godot overlay that draws her (one program since 2026-10-16 -
//! the logic is the `felt-*` crates under `crates/`). It all runs on one thread of its own, single-threaded
//! like the Python event loop it was ported from; the rest of felt-tray talks to it through a [`Handle`].
//! This file only wires the parts together.

mod assets;
mod ctx;
mod lipsync;
mod overlay;
mod reply;
mod speech;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use felt_state::{BehaviorState, MonotonicClock};
use tokio::sync::mpsc;

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
    pub fn reply(&self, text: &str, audio: Vec<u8>, tag: Option<String>, emotion: Option<String>) {
        let text = text.to_owned();
        let _ = self.tx.send(Msg::Reply {
            text,
            audio,
            tag,
            emotion,
        });
    }

    pub fn more(&self, text: &str, audio: Vec<u8>) {
        let text = text.to_owned();
        let _ = self.tx.send(Msg::More { text, audio });
    }
}

/// Starts her body's thread: the overlay, its link, and her reactions to what felt-tray tells it.
pub fn start(cfg: &config::Body, core: Arc<FeltCore>, player: Player) -> Result<Handle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let cfg = cfg.clone();
    std::thread::Builder::new()
        .name("felt-body".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("her body's runtime");
            tokio::task::LocalSet::new().block_on(&rt, run(cfg, core, player, rx));
        })
        .context("can't start her body's thread")?;
    Ok(Handle { tx })
}

async fn run(
    cfg: config::Body,
    core: Arc<FeltCore>,
    player: Player,
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
    });
    let (sentences, queue) = mpsc::unbounded_channel();
    tokio::task::spawn_local(speech::run(ctx.clone(), queue));
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Thinking => {
                ctx.next_generation();
                ctx.overlay.lipsync(&[]).await; // a cut-off reply's mouth eases closed
                ctx.behavior
                    .begin_thinking(ctx.assets.thinking_tag(), CLIP_TIMEOUT_S);
            }
            Msg::Reply {
                text,
                audio,
                tag,
                emotion,
            } => {
                let generation = ctx.next_generation();
                ctx.bubble.replace(text.clone());
                let reaction = Some(reply::begin(&ctx, &text, tag, emotion));
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
