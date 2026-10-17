//! The Felt app's quiet switch, as on Windows (WP-42.5): she's cut off, says "okay okay, I'll zip it" (one of
//! her recorded presence lines, when `[body] sounds` points at them), and goes off screen until it's switched
//! off; nothing she'd say on her own plays meanwhile. Being hushed annoys her a little (mood). Switched off,
//! she walks back in and has something to say about it: felt-core's `unhushed` line, else a recorded one.

use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::ctx::Ctx;
use super::{lines, presence, react, speech};

/// How long to wait for her to be back on screen before the un-hush line.
const BACK_WAIT: Duration = Duration::from_secs(30);

/// The hush shortcut / "shush for 10 minutes": hushed for `minutes` (the shortcut again, or the time running
/// out, ends it - with her line about it). Unlike the app's quiet switch she stays on screen.
pub async fn for_minutes(ctx: &Rc<Ctx>, minutes: f64) {
    set(ctx, true).await;
    let c = ctx.clone();
    let timer = tokio::task::spawn_local(async move {
        tokio::time::sleep(Duration::from_secs_f64(minutes.max(0.0) * 60.0)).await;
        c.hush_timer.replace(None);
        set(&c, false).await;
    });
    if let Some(old) = ctx.hush_timer.replace(Some(timer)) {
        old.abort();
    }
}

/// Lifts a hush now (the shortcut's second press, "you can talk again").
pub async fn lift(ctx: &Rc<Ctx>) {
    if let Some(t) = ctx.hush_timer.replace(None) {
        t.abort();
    }
    set(ctx, false).await;
}

pub async fn set(ctx: &Rc<Ctx>, quiet: bool) {
    if quiet == ctx.quiet.replace(quiet) {
        return;
    }
    if !quiet {
        let secs = ctx.hushed_at.take().map_or(0, |t| t.elapsed().as_secs());
        tracing::info!("hush: lifted after {secs}s");
        tokio::task::spawn_local(announce_unhush(ctx.clone(), secs));
        return;
    }
    tracing::info!("hush: on");
    ctx.hushed_at.set(Some(Instant::now()));
    ctx.overlay.lipsync(&[]).await; // her mouth eases closed with the cut-off sentence
    ctx.mood.borrow_mut().on_hush();
    ctx.events.report("hush", json!({}));
    if let Some(wav) = lines::presence_line(ctx, "zip_it") {
        ctx.player.play(wav);
    }
}

/// Once she's back on screen: felt-core's `unhushed` line (it decides; "ten whole minutes..."), else a
/// recorded `unhush` take, so lifting it is never met with silence.
async fn announce_unhush(ctx: Rc<Ctx>, hushed_for_s: u64) {
    let deadline = Instant::now() + BACK_WAIT;
    while !presence::settled_visible(&ctx) {
        if Instant::now() >= deadline || ctx.quiet.get() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let details = json!({ "hushed_for_s": hushed_for_s, "planned_s": 0, "early": true });
    match ctx
        .core
        .react_event("unhushed", &details, &react::tone(&ctx))
        .await
    {
        Ok(Some(r)) => {
            tracing::info!("hush: un-hush line, {} chars", r.text.len());
            speech::say(&ctx, r.text, r.audio, r.tag, r.emotion, false);
        }
        other => {
            if let Err(e) = other {
                tracing::info!("hush: no un-hush line from felt-core ({e:#})");
            }
            if let Some(wav) = lines::presence_line(&ctx, "unhush") {
                ctx.player.play(wav);
            }
        }
    }
}
