//! Hide / show (WP-10, the Felt app's hide switch): she waves, walks out through the nearer outer edge of the
//! desktop and disappears; shown again, she reappears past that edge and walks back in to the middle of that
//! monitor. A newer hide/show supersedes a walk still in flight (a cancelled walk-off never hides her later).
//! A line you send while she's hidden brings her back.

use std::rc::Rc;

use felt_presence::{ARRIVING, LEAVING, VISIBLE};
use serde_json::json;

use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::moves::{self, RUN_TAG, WALK_TAG};

pub type Presence = felt_presence::PresenceState<fn() -> f64>;

/// Is she on screen (or on her way in)? Ambient behaviour and unprompted lines only run then.
pub fn visible(ctx: &Ctx) -> bool {
    let p = ctx.presence.borrow();
    p.state == VISIBLE || p.state == ARRIVING
}

/// Fully back on screen (the walk in has finished).
pub fn settled_visible(ctx: &Ctx) -> bool {
    ctx.presence.borrow().state == VISIBLE
}

pub fn hide(ctx: &Rc<Ctx>) {
    let outcome = ctx.presence.borrow_mut().request_hide();
    tracing::info!("presence: hide ({outcome})");
    if outcome == "walk_off" {
        start(ctx, walk_off(ctx.clone()));
    }
}

pub fn show(ctx: &Rc<Ctx>) {
    let was_leaving = ctx.presence.borrow().state == LEAVING;
    if ctx.presence.borrow_mut().request_show() {
        tracing::info!("presence: show");
        start(ctx, walk_on(ctx.clone(), was_leaving));
    }
}

/// A new transition supersedes the one in flight.
fn start(ctx: &Rc<Ctx>, walk: impl Future<Output = ()> + 'static) {
    if let Some(old) = ctx
        .presence_walk
        .replace(Some(tokio::task::spawn_local(walk)))
    {
        old.abort();
    }
}

async fn walk_off(ctx: Rc<Ctx>) {
    if ctx.assets.tags.contains("gesture/wave") {
        ctx.behavior
            .run_sequence(&["gesture/wave"], CLIP_TIMEOUT_S, None)
            .await;
    }
    let g = moves::geometry(&ctx);
    let (left, right) = (g.offscreen_left_x(), g.offscreen_right_x());
    let x = ctx.current_x.get();
    let exit = if (left - x).abs() <= (right - x).abs() {
        left
    } else {
        right
    };
    ctx.exit_left.set(exit > 0.0); // world +x is screen left
    moves::walk_to(&ctx, exit, cycle_tag(&ctx), true).await;
    if ctx.presence.borrow().state != LEAVING {
        return; // shown again mid-walk: never hide her now
    }
    ctx.overlay.send(&json!({ "visible": false })).await;
    ctx.overlay.send(&json!({ "bubble": null })).await;
    ctx.presence.borrow_mut().mark_offscreen();
}

async fn walk_on(ctx: Rc<Ctx>, already_visible: bool) {
    let g = moves::geometry(&ctx);
    let entry = if ctx.exit_left.get() {
        g.offscreen_left_x()
    } else {
        g.offscreen_right_x()
    };
    if !already_visible {
        ctx.current_x.set(entry);
        ctx.overlay.move_to(entry, true, false).await;
    }
    ctx.overlay.send(&json!({ "visible": true })).await;
    let landing = match g.view_at_world_x(g.clamp_roam(entry)) {
        Some(v) => {
            let (lo, hi) = g.view_span(v);
            (lo + hi) / 2.0
        }
        None => g.clamp_roam(-entry * 0.5),
    };
    moves::walk_to(&ctx, landing, cycle_tag(&ctx), false).await;
    ctx.presence.borrow_mut().mark_onscreen();
}

fn cycle_tag(ctx: &Ctx) -> &'static str {
    if ctx.assets.tags.contains(WALK_TAG) {
        WALK_TAG
    } else {
        RUN_TAG
    }
}
