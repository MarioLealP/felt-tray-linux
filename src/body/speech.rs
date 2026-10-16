//! Her sentences, in order: mouth cues from Rhubarb, then onto the voice player; the moment a sentence really
//! starts playing, the cues go to the overlay (so her mouth starts with the sound, not with our guess of it).
//! The first sentence of a reply also brings her reaction (gesture + face).

use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::ctx::{BUBBLE_MIN_S, CLIP_TIMEOUT_S, Ctx};
use super::lipsync;
use super::reply::Pending;

/// How long past a sentence's last cue she still counts as speaking (the gap to the next streamed one).
const SPEAKING_SLACK_S: f64 = 1.5;

pub struct Sentence {
    pub generation: u64,
    pub text: String,
    pub audio: Vec<u8>,
    /// The first sentence of a reply: its reaction.
    pub reaction: Option<Pending>,
}

pub async fn run(ctx: Rc<Ctx>, mut rx: mpsc::UnboundedReceiver<Sentence>) {
    while let Some(s) = rx.recv().await {
        if !ctx.current(s.generation) {
            continue; // a newer reply took over
        }
        let cues = lipsync::cues(&ctx.rhubarb, &s.audio, &s.text).await;
        let speaking_s = cues.last().and_then(|c| c["end"].as_f64()).unwrap_or(0.0);
        let clips = match s.reaction {
            Some(r) => Some(r.settle(&ctx, speaking_s).await),
            None => None,
        };
        if !ctx.current(s.generation) {
            continue;
        }
        let started = ctx.player.play_then(s.audio);
        tokio::task::spawn_local(speak(ctx.clone(), started, cues, speaking_s, clips));
    }
}

async fn speak(
    ctx: Rc<Ctx>,
    started: oneshot::Receiver<()>,
    cues: Vec<Value>,
    speaking_s: f64,
    clips: Option<Vec<String>>,
) {
    if started.await.is_err() {
        return; // stopped before it played
    }
    ctx.overlay.lipsync(&cues).await;
    ctx.speaking_until
        .set(Instant::now() + Duration::from_secs_f64(speaking_s + SPEAKING_SLACK_S));
    let bubble = ctx.bubble.borrow().clone();
    ctx.overlay
        .bubble(&bubble, BUBBLE_MIN_S.max(speaking_s + 1.5))
        .await;
    match clips {
        Some(clips) => {
            let tags: Vec<&str> = clips.iter().map(String::as_str).collect();
            ctx.behavior.run_sequence(&tags, CLIP_TIMEOUT_S, None).await;
        }
        None => {
            // a later sentence: keep her face up while she's still talking
            let pose = ctx.pose.borrow().clone();
            ctx.apply_pose(&pose, speaking_s + 1.5).await;
        }
    }
}
