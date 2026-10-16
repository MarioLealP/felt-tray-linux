//! Sitting on a ledge (WP-23 slice 5): after a wander lands her on a window top she may sit on its edge - more
//! likely when sleepy, less when bored (felt-ledge-sit's chance) - for 45-180 s (shorter when bored), then stand
//! up. Never on a floor. If the window goes while she sits, main.gd stands her up itself (world.rs tidies up).

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

use felt_ledge_sit::{
    BORED_DURATION_SCALE, BORED_DURATION_THRESHOLD, SEATED_ACK_TIMEOUT_S, SIT_DURATION_MAX_S,
    SIT_DURATION_MIN_S,
};
use felt_world::Platform;
use rand::RngExt;
use serde_json::json;

use super::ctx::Ctx;

/// Once a wander has landed her on `platform`: a mood-weighted chance she sits (`force`: always).
pub async fn maybe_sit(
    ctx: &Rc<Ctx>,
    platform: &Platform,
    mood: &BTreeMap<String, f64>,
    force: bool,
) {
    if platform.kind == "floor" {
        return;
    }
    let m: HashMap<String, f64> = mood.clone().into_iter().collect();
    if !force && rand::rng().random::<f64>() >= felt_ledge_sit::sit_chance(&m) {
        return;
    }
    let ack = Duration::from_secs_f64(SEATED_ACK_TIMEOUT_S);
    let seated = {
        let _legs = ctx.behavior.hold_base().await;
        ctx.overlay.ledge_sit(platform.y, ack).await
    };
    if !seated {
        return;
    }
    tracing::info!("ledge: sitting on {}", platform.id);
    ctx.events.report("ledge_sit", json!({}));
    let stay = stay_s(mood);
    let c = ctx.clone();
    let timer = tokio::task::spawn_local(async move {
        tokio::time::sleep(Duration::from_secs_f64(stay)).await;
        stand(&c, false).await;
    });
    let mut w = ctx.world.borrow_mut();
    w.seated_on = Some(platform.id.clone());
    if let Some(old) = w.sit_timer.replace(timer) {
        old.abort();
    }
}

/// Stands her up off the ledge if she's sitting on one (something needs her legs before the stay ran out).
pub async fn stand_up(ctx: &Ctx) {
    stand(ctx, true).await;
}

/// `cancel_timer`: false when the stay timer itself calls this (aborting it would cut this short).
async fn stand(ctx: &Ctx, cancel_timer: bool) {
    let timer = {
        let mut w = ctx.world.borrow_mut();
        if w.seated_on.take().is_none() {
            return;
        }
        w.sit_timer.take()
    };
    if let Some(t) = timer
        && cancel_timer
    {
        t.abort();
    }
    let _legs = ctx.behavior.hold_base().await;
    ctx.overlay
        .ledge_stand(Duration::from_secs_f64(SEATED_ACK_TIMEOUT_S))
        .await;
    tracing::info!("ledge: stood up");
}

fn stay_s(mood: &BTreeMap<String, f64>) -> f64 {
    let (mut lo, mut hi) = (SIT_DURATION_MIN_S, SIT_DURATION_MAX_S);
    if mood.get("boredom").copied().unwrap_or(0.0) >= BORED_DURATION_THRESHOLD {
        lo *= BORED_DURATION_SCALE;
        hi *= BORED_DURATION_SCALE;
    }
    rand::rng().random_range(lo..hi)
}
