//! What you ask of her body directly - the shortcuts, and the desktop actions in felt-core's replies ("be right
//! back", "come here", "shush for ten minutes"): listening while you talk, hide/show, be right back (away 5 /
//! 15 / 30 / 60 min, then she comes back with a "back!"), shoo, come to the cursor, repeat the last line, hush.

use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};

use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::moves::{self, RUN_TAG, WALK_TAG};
use super::{hush, lines, presence};

/// While you talk: her listening loop, a curious brow, the head tilt. Then she stops and starts thinking.
pub async fn listening(ctx: &Rc<Ctx>, on: bool) {
    if on {
        if !presence::visible(ctx) {
            presence::show(ctx); // talking to her while she's away brings her back
        }
        ctx.next_generation();
        let tag = felt_library::resolve_known_tag(Some("reaction/listening"), &ctx.assets.tags)
            .unwrap_or("idle");
        ctx.behavior.begin_thinking(tag, CLIP_TIMEOUT_S);
        ctx.overlay
            .face(&json!({ "brows": { "Fcl_BRW_Surprised": 0.3 } }), 0.0)
            .await;
        ctx.overlay.send(&json!({ "listening": true })).await;
    } else {
        ctx.overlay.send(&json!({ "listening": false })).await;
        ctx.behavior.end_thinking_and_wait().await;
        ctx.overlay.face(&json!({ "brows": {} }), 0.0).await;
    }
}

/// One named action (the shortcut ids, or felt-core's `presence` ui_action).
pub async fn act(ctx: &Rc<Ctx>, what: &str, payload: &Value) {
    tracing::info!("action: {what}");
    match what {
        "hide" if presence::visible(ctx) => presence::hide(ctx),
        "hide" | "show" | "summon" => presence::show(ctx),
        "brb" => be_right_back(ctx),
        "move" => shoo(ctx).await,
        "come" => come_here(ctx).await,
        "repeat" => repeat(ctx).await,
        "hush" => hush::for_minutes(ctx, payload["minutes"].as_f64().unwrap_or(10.0)).await,
        "unhush" => hush::lift(ctx).await,
        // the shortcut toggles: hushed -> talk again, else hush for ten minutes
        "stop" if ctx.quiet.get() => hush::lift(ctx).await,
        "stop" => {
            ctx.player.stop(); // cut off mid-sentence
            hush::for_minutes(ctx, 10.0).await;
        }
        other => tracing::info!("action: nothing to do for '{other}'"),
    }
}

/// Away with a timer that climbs 5 -> 15 -> 30 -> 60 min on repeated presses; back with a "back!" and a wave.
fn be_right_back(ctx: &Rc<Ctx>) {
    let (outcome, minutes) = ctx.presence.borrow_mut().request_brb();
    tracing::info!("presence: brb {minutes} min ({outcome})");
    if outcome == "walk_off" {
        presence::hide_walk(ctx);
    }
}

/// From the presence loop: an away timer ran out.
pub async fn back_from_brb(ctx: Rc<Ctx>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let expired = ctx.presence.borrow_mut().check_and_consume_expiry();
        if !expired {
            continue;
        }
        presence::show(&ctx);
        while !presence::settled_visible(&ctx) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if let Some(wav) = lines::presence_line(&ctx, "back") {
            ctx.player.play(wav);
        }
        if let Some(tag) = ["greeting", "gesture/wave"]
            .into_iter()
            .find(|t| ctx.assets.tags.contains(*t))
        {
            ctx.behavior
                .run_sequence(&[tag], CLIP_TIMEOUT_S, None)
                .await;
        }
    }
}

/// Shoo: off to whichever end of her floor is farther from the cursor.
async fn shoo(ctx: &Ctx) {
    let g = moves::geometry(ctx);
    let cursor = ctx
        .cursor
        .get()
        .filter(|_| g.has_pixel_map())
        .map(|(x, _)| g.px_to_world_x(x));
    let target = felt_presence::pick_move_target(ctx.current_x.get(), g.roam(), 0.5, 0.5, cursor);
    moves::walk_to(ctx, target, cycle(ctx), false).await;
}

/// Come here: to the cursor, wherever it is on her floor.
async fn come_here(ctx: &Ctx) {
    let g = moves::geometry(ctx);
    let target = match ctx.cursor.get() {
        Some((x, _)) if g.has_pixel_map() => g.cursor_px_to_world_x(x),
        _ => ctx.current_x.get(),
    };
    moves::walk_to(ctx, target, cycle(ctx), false).await;
}

/// Her last line again (her mouth with it; no reaction clip - she already did that).
async fn repeat(ctx: &Ctx) {
    let Some((wav, cues)) = ctx.last_reply.borrow().clone() else {
        tracing::info!("action: nothing to repeat yet");
        return;
    };
    let started = ctx.player.play_then(wav);
    if started.await.is_ok() {
        ctx.overlay.lipsync(&cues).await;
    }
}

fn cycle(ctx: &Ctx) -> &'static str {
    if ctx.assets.tags.contains(WALK_TAG) {
        WALK_TAG
    } else {
        RUN_TAG
    }
}
