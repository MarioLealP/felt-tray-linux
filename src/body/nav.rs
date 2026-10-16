//! Getting somewhere else in her world (WP-23): a route of walks, hops down and climbs (felt-navigation plans
//! it) run step by step under one hold of her legs, each waiting for main.gd's real `arrived` / `landed` /
//! `climbed`. And where an ambient wander goes when it picks a ledge: further and higher when she's bored,
//! nearer and lower when sleepy, a little pull toward the cursor.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use felt_navigation::{Climb, HopDown, Step, Walk};
use felt_world::Platform;
use rand::RngExt;

use super::ctx::Ctx;
use super::moves::{self, WALK_TAG};
use super::world;

const STEP_TIMEOUT_S: f64 = 8.0;
/// How often an ambient wander picks a ledge instead of a floor spot.
pub const PLATFORM_ROAM_CHANCE: f64 = 0.35;
const CURSOR_PULL: f64 = 0.5;
const FOCUSED_PENALTY: f64 = 0.3;
const FOCUSED_RADIUS_PX: f64 = 200.0;

/// True if every step completed in time.
pub async fn run_plan(ctx: &Ctx, plan: &[Step]) -> bool {
    moves::stand_up_if_sitting(ctx).await;
    let _legs = ctx.behavior.hold_base().await;
    for step in plan {
        if !run_step(ctx, step).await {
            tracing::warn!("navigation: plan aborted at {step:?}");
            return false;
        }
    }
    true
}

async fn run_step(ctx: &Ctx, step: &Step) -> bool {
    let timeout = Duration::from_secs_f64(STEP_TIMEOUT_S);
    match step {
        Step::Walk(Walk { x_px }) => {
            // walk_body, not walk_to: this plan already holds her legs
            let x = moves::geometry(ctx).px_to_world_x(*x_px);
            ctx.current_x.set(x);
            let looped = ctx.assets.tags.contains(WALK_TAG).then_some(WALK_TAG);
            ctx.behavior
                .walk_body(x, looped, false, None, None, false, STEP_TIMEOUT_S)
                .await;
            true
        }
        Step::HopDown(HopDown { to_platform, .. }) => {
            let Some(y) = target_y(ctx, to_platform) else {
                return false;
            };
            ctx.overlay.hop(y, timeout).await.is_some()
        }
        Step::Climb(Climb {
            to_platform,
            meters,
            ..
        }) => {
            let Some(y) = target_y(ctx, to_platform) else {
                return false;
            };
            ctx.overlay.climb(y, *meters, timeout).await.is_some()
        }
    }
}

/// The step's platform (gone = a window closed mid-plan: nothing sane to do), marked as where she'll land.
fn target_y(ctx: &Ctx, id: &str) -> Option<f64> {
    let mut w = ctx.world.borrow_mut();
    let y = w.platform(id)?.y;
    w.pending = Some(id.to_owned());
    Some(y)
}

/// The explicit "go sit on that ledge" (Mario 2026-09-27: "need a way to force her onto a ledge"): the ledge
/// nearest the cursor she can route to, then she always sits.
pub async fn go_to_ledge(ctx: &Rc<Ctx>) {
    let route = {
        let w = ctx.world.borrow();
        let Some(current) = w.current.clone() else {
            tracing::info!("ledge: she isn't anchored yet");
            return;
        };
        let g = moves::geometry(ctx);
        let her_x = g.world_to_px_x(ctx.current_x.get());
        let here_y = w.platform(&current).map_or(0.0, |p| p.y);
        let point = ctx.cursor.get().unwrap_or((her_x, here_y));
        let ppm = if g.ppm > 0.0 { g.ppm } else { 1.0 };
        let nav = world::nav_platforms(&w.platforms);
        felt_navigation::ledges_near(&nav, point, Some(&current), her_width_px(ctx))
            .into_iter()
            .find_map(|(ledge, x)| {
                let plan = felt_navigation::plan_route(&nav, ppm, &current, her_x, &ledge.id, x)?;
                Some((plan, w.platform(&ledge.id)?.clone()))
            })
    };
    let Some((plan, ledge)) = route else {
        tracing::info!("ledge: no ledge she can reach near the cursor");
        return;
    };
    tracing::info!("ledge: going to {} ({} steps)", ledge.id, plan.len());
    if run_plan(ctx, &plan).await {
        let mood = ctx.mood.borrow().as_dict();
        super::ledge::maybe_sit(ctx, &ledge, &mood, true).await;
    }
}

/// A route from where she stands to a mood-weighted ledge, and that ledge. None when there's nothing wide
/// enough, she isn't anchored yet, or there's no route.
pub fn plan_ledge_walk(ctx: &Ctx, mood: &BTreeMap<String, f64>) -> Option<(Vec<Step>, Platform)> {
    let w = ctx.world.borrow();
    let current = w.current.clone()?;
    let g = moves::geometry(ctx);
    let width = her_width_px(ctx);
    let candidates: Vec<&Platform> = w
        .platforms
        .iter()
        .filter(|p| p.kind != "floor" && p.x1 - p.x0 >= width)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let her_x = g.world_to_px_x(ctx.current_x.get());
    let cursor_x = ctx.cursor.get().map(|(x, _)| x);
    // the top-most window's ledge stands in for the focused window's (felt-tray lists windows top-first)
    let focused = w
        .platforms
        .iter()
        .find(|p| p.kind == "window")
        .map(|p| (p.x0 + p.x1) / 2.0);
    let weights: Vec<f64> = candidates
        .iter()
        .map(|p| roam_weight(p, her_x, cursor_x, focused, mood))
        .collect();
    let picked = (*pick_weighted(&candidates, &weights)?).clone();
    let margin = width / 2.0 + 5.0;
    let (lo, hi) = (picked.x0 + margin, picked.x1 - margin);
    let x = if hi > lo {
        rand::rng().random_range(lo..hi)
    } else {
        (picked.x0 + picked.x1) / 2.0
    };
    let ppm = if g.ppm > 0.0 { g.ppm } else { 1.0 };
    let plan = felt_navigation::plan_route(
        &world::nav_platforms(&w.platforms),
        ppm,
        &current,
        her_x,
        &picked.id,
        x,
    )?;
    Some((plan, picked))
}

/// A qualitative score, not physics: bored -> further and higher, sleepy -> nearer and lower, a small pull
/// toward the cursor, and a penalty for the focused window's own ledge.
fn roam_weight(
    p: &Platform,
    her_x: f64,
    cursor_x: Option<f64>,
    focused_x: Option<f64>,
    mood: &BTreeMap<String, f64>,
) -> f64 {
    let center = (p.x0 + p.x1) / 2.0;
    let distance = (center - her_x).abs();
    let height = (1200.0 - p.y).max(0.0);
    let boredom = mood.get("boredom").copied().unwrap_or(0.0);
    let energy = mood.get("energy").copied().unwrap_or(0.5);
    let sleepy = (0.35 - energy).max(0.0) / 0.35;
    let reach = distance / 500.0 + height / 500.0;
    let mut weight = 1.0 + boredom * reach - sleepy * reach * 0.8;
    if let Some(cx) = cursor_x {
        weight += CURSOR_PULL / (1.0 + (center - cx).abs() / 300.0);
    }
    if focused_x.is_some_and(|f| (center - f).abs() < FOCUSED_RADIUS_PX) {
        weight -= FOCUSED_PENALTY;
    }
    weight.max(0.05)
}

fn pick_weighted<'a, T>(items: &'a [T], weights: &[f64]) -> Option<&'a T> {
    let total: f64 = weights.iter().sum();
    if items.is_empty() || total <= 0.0 {
        return None;
    }
    let mut r = rand::rng().random_range(0.0..total);
    for (item, w) in items.iter().zip(weights) {
        if r < *w {
            return Some(item);
        }
        r -= w;
    }
    items.last()
}

/// Her width in desktop px (main.gd's ppm x her width), or a guess from her height.
fn her_width_px(ctx: &Ctx) -> f64 {
    let g = ctx.overlay.geometry();
    let ppm = g.as_ref().and_then(|g| g["ppm"].as_f64());
    let width = g.as_ref().and_then(|g| g["character_width"].as_f64());
    match (ppm, width) {
        (Some(p), Some(w)) if p > 0.0 && w > 0.0 => p * w,
        _ => world::her_height_px(ctx) * 0.77,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledge(x0: f64, x1: f64, y: f64) -> Platform {
        Platform {
            id: "w".into(),
            hwnd: Some(1),
            x0,
            x1,
            y,
            kind: "window".into(),
        }
    }

    #[test]
    fn boredom_favours_far_high_ledges_and_sleepiness_near_low_ones() {
        let far_high = ledge(2000.0, 2400.0, 200.0);
        let near_low = ledge(0.0, 400.0, 1100.0);
        let bored: BTreeMap<String, f64> = [("boredom".into(), 1.0), ("energy".into(), 0.6)].into();
        let sleepy: BTreeMap<String, f64> =
            [("boredom".into(), 0.0), ("energy".into(), 0.05)].into();
        let w = |p: &Platform, m| roam_weight(p, 200.0, None, None, m);
        assert!(w(&far_high, &bored) > w(&near_low, &bored));
        assert!(w(&near_low, &sleepy) > w(&far_high, &sleepy));
        assert!(w(&far_high, &sleepy) >= 0.05);
    }

    #[test]
    fn weighted_pick_only_returns_items() {
        let items = [1, 2, 3];
        for _ in 0..100 {
            assert!(items.contains(pick_weighted(&items, &[0.1, 5.0, 1.0]).unwrap()));
        }
        assert!(pick_weighted::<i32>(&[], &[]).is_none());
    }
}
