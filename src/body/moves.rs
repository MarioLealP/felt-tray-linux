//! Her legs: walking somewhere, sitting down, standing up - the shared moves the ambient loops (and later
//! presence and props) use. Where she can go comes from the overlay's own geometry report.

use felt_geometry::OverlayGeometry;
use serde_json::Value;

use super::ctx::{CLIP_TIMEOUT_S, Ctx};

pub const WALK_TAG: &str = "locomotion/walk/normal_walk";
pub const RUN_TAG: &str = "locomotion/run";
const SIT_TO_STAND: &str = "transition/sit_to_stand";
const STAND_TO_SIT: &str = "transition/stand_to_sit";
pub const SITTING_IDLE: &str = "idle/sitting";

/// main.gd's reported frame (her roam limits, the monitors' world spans), or a rough guess until it reports.
pub fn geometry(ctx: &Ctx) -> OverlayGeometry {
    let fallback = OverlayGeometry::new(3.0, 2.0, 0.66);
    felt_geometry::from_overlay_report(&ctx.overlay.geometry().unwrap_or(Value::Null), fallback)
}

/// Walks to world `x` looping `cycle_tag` (walk or run) until main.gd says she arrived; stands up first.
pub async fn walk_to(ctx: &Ctx, x: f64, cycle_tag: &str) {
    stand_up_if_sitting(ctx).await;
    ctx.current_x.set(x);
    let looped = ctx.assets.tags.contains(cycle_tag).then_some(cycle_tag);
    let running = cycle_tag == RUN_TAG;
    ctx.behavior
        .walk_to(x, looped, running, None, None, false, CLIP_TIMEOUT_S)
        .await;
}

pub async fn stand_up_if_sitting(ctx: &Ctx) {
    if !ctx.behavior.sitting() {
        return;
    }
    if ctx.assets.tags.contains(SIT_TO_STAND) {
        ctx.behavior
            .run_sequence(&[SIT_TO_STAND], CLIP_TIMEOUT_S, None)
            .await;
    }
    ctx.behavior.set_posture("standing");
    // otherwise idle/sitting can flash between the stand-up and what comes next
    if ctx.assets.tags.contains("idle") {
        ctx.overlay.play_tag("idle", false, None).await;
    }
}

/// Sits down on the floor where she is (when this desktop has the clips). True if she sat.
pub async fn sit_down(ctx: &Ctx) -> bool {
    let tags = &ctx.assets.tags;
    if !(tags.contains(STAND_TO_SIT) && tags.contains(SITTING_IDLE)) {
        return false;
    }
    ctx.behavior
        .run_sequence(&[STAND_TO_SIT], CLIP_TIMEOUT_S, None)
        .await;
    ctx.behavior.set_posture("sitting");
    // looping: sent raw after the transition (it never sends `finished`)
    ctx.overlay.play_tag(SITTING_IDLE, false, None).await;
    true
}
