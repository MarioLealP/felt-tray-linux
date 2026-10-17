//! Her sentences, in order: mouth cues from Rhubarb, then onto the voice player; the moment a sentence really
//! starts playing, the cues go to the overlay (so her mouth starts with the sound, not with our guess of it).
//! The first sentence of a reply also brings her reaction (gesture + face).

use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::ctx::{BUBBLE_MIN_S, CLIP_TIMEOUT_S, Ctx};
use super::lipsync;
use super::presence;
use super::reply::{self, Pending};

/// How many recent emotions her tone line looks at.
const STREAK_LEN: usize = 5;

/// How long past a sentence's last cue she still counts as speaking (the gap to the next streamed one).
const SPEAKING_SLACK_S: f64 = 1.5;

pub struct Sentence {
    pub generation: u64,
    pub text: String,
    pub audio: Vec<u8>,
    /// The first sentence of a reply: its reaction.
    pub reaction: Option<Pending>,
}

/// A reply's first (or only) sentence: she says it with a reaction. `prompted` = you asked for it. While she's
/// hidden an unprompted line is dropped; one you asked for brings her back.
pub fn say(
    ctx: &Rc<Ctx>,
    text: String,
    audio: Vec<u8>,
    tag: Option<String>,
    emotion: Option<String>,
    prompted: bool,
) {
    if !presence::visible(ctx) {
        if !prompted {
            return;
        }
        presence::show(ctx);
    }
    if !prompted && ctx.quiet.get() {
        // hushed: what she'd say on her own shows as a bubble, without her voice
        let c = ctx.clone();
        tokio::task::spawn_local(async move { c.overlay.bubble(&text, BUBBLE_MIN_S).await });
        return;
    }
    if let Some(f) = ctx.filler.take() {
        f.abort(); // the reply is here: no "hmm"
    }
    let generation = ctx.next_generation();
    ctx.bubble.replace(text.clone());
    if let Some(e) = &emotion {
        let mut streak = ctx.streak.borrow_mut();
        streak.push(e.clone());
        let extra = streak.len().saturating_sub(STREAK_LEN);
        streak.drain(..extra);
    }
    let reaction = Some(reply::begin(ctx, &text, tag, emotion, prompted));
    let _ = ctx.sentences.send(Sentence {
        generation,
        text,
        audio,
        reaction,
    });
}

/// A later sentence of the current reply.
pub fn more(ctx: &Ctx, text: String, audio: Vec<u8>) {
    let _ = ctx.sentences.send(Sentence {
        generation: ctx.generation.get(),
        text,
        audio,
        reaction: None,
    });
}

pub async fn run(ctx: Rc<Ctx>, mut rx: mpsc::UnboundedReceiver<Sentence>) {
    while let Some(s) = rx.recv().await {
        if !ctx.current(s.generation) {
            continue; // a newer reply took over
        }
        let cues = lipsync::cues(&ctx.rhubarb, &s.audio, &s.text).await;
        let speaking_s = cues.last().and_then(|c| c["end"].as_f64()).unwrap_or(0.0);
        if s.reaction.is_some() {
            ctx.last_reply
                .replace(Some((s.audio.clone(), cues.clone())));
        }
        let clips = match s.reaction {
            Some(r) => Some(r.settle(&ctx, speaking_s).await),
            None => None,
        };
        if !ctx.current(s.generation) {
            continue;
        }
        let wav = s.audio.clone();
        let started = ctx.player.play_then(s.audio);
        tokio::task::spawn_local(speak(ctx.clone(), started, cues, speaking_s, clips, wav));
    }
}

async fn speak(
    ctx: Rc<Ctx>,
    started: oneshot::Receiver<()>,
    cues: Vec<Value>,
    speaking_s: f64,
    clips: Option<Vec<String>>,
    wav: Vec<u8>,
) {
    if started.await.is_err() {
        return; // stopped before it played
    }
    let pose = ctx.pose.borrow().clone();
    tokio::task::spawn_local(super::face::accents(ctx.clone(), wav, pose));
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
