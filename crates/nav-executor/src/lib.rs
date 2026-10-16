//! WP-23 slice 3: runs a navigation plan against the real overlay, one
//! step at a time - `Walk` through the ordinary walk_to (WP-18), `HopDown`/
//! `Climb` through the {"hop"}/{"climb"} messages, awaiting each step's
//! real completion notice (never a guessed duration, same discipline as
//! overlay.py's play_and_wait/wait_for_arrived).
//!
//! Holds the BASE lock for the whole plan (state.py's
//! BehaviorState::hold_base) - a multi-step traversal is one atomic
//! sequence, same reasoning as WP-18's walk_to holding it for a single
//! walk. That's also what keeps loops._sync_platform_support's automatic
//! "never floating" glue out of the way while a plan is in flight (it
//! skips while state.behavior.base_busy).
//!
//! Each step gets STEP_TIMEOUT_S. A step that never completes aborts the
//! whole plan rather than pressing on with stale assumptions about where
//! she is - loops.world_loop's own next tick re-derives
//! current_platform_id from wherever she actually ended up (this module
//! doesn't need to guess it).
//!
//! The other felt_animation modules this drives (navigation's Step types,
//! state's BehaviorState, the overlay protocol, behavior's stand-up path)
//! are NOT ported here - minimal stand-ins for them live in [`shared`].

use std::collections::HashMap;

pub mod shared;
pub use shared::*;

/// Per-step safety net passed to the overlay's waiters - a step that never
/// completes aborts the whole plan.
pub const STEP_TIMEOUT_S: f64 = 8.0;

/// True if every step in `plan` completed within its timeout.
pub async fn run_plan<O, H, G>(state: &AppState<O, H, G>, plan: &[Step]) -> bool
where
    O: Overlay,
    H: StandUpHooks,
    G: Geometry,
{
    // same as behavior.walk_to - can't walk seated
    stand_up_if_sitting(state).await;
    let _guard = state.behavior.hold_base().await;
    for step in plan {
        if !run_step(state, step).await {
            // "navigation: plan aborted, step didn't complete" - the
            // Python logs the step here; abort rather than press on with
            // stale assumptions about where she is.
            return false;
        }
    }
    true
}

async fn run_step<O, H, G>(state: &AppState<O, H, G>, step: &Step) -> bool
where
    O: Overlay,
    H: StandUpHooks,
    G: Geometry,
{
    match step {
        Step::Walk(walk) => {
            // walk_body, NOT behavior.walk_to/state.behavior.walk_to -
            // run_plan already holds the base lock for the whole plan
            // (module docstring); walk_to would try to re-acquire the same
            // (non-reentrant) lock and deadlock. Known gap: unlike
            // HopDown/Climb below, walk_body doesn't report whether
            // {"arrived"} actually came back in time - a stuck walk step
            // logs overlay.py's own timeout warning but still counts as
            // "completed" here rather than aborting the plan.
            let x = state.geometry.px_to_world_x(walk.x_px);
            state.current_x.set(x);
            let loop_tag = if state.known_tags.contains(AMBIENT_WALK_TAG) {
                Some(AMBIENT_WALK_TAG)
            } else {
                None
            };
            state
                .behavior
                .walk_body(x, loop_tag, false, None, None, false, STEP_TIMEOUT_S)
                .await;
            true
        }
        Step::HopDown(hop) => {
            let Some(target) = target_platform(state, &hop.to_platform) else {
                // The platform this step aims for is gone from the latest
                // world snapshot (a window closed/moved mid-plan) -
                // nothing sane to send.
                return false;
            };
            *state.pending_platform_id.borrow_mut() = Some(hop.to_platform.clone());
            state.overlay.hop(target.y).await;
            state
                .overlay
                .wait_for_landed(STEP_TIMEOUT_S)
                .await
                .is_some()
        }
        Step::Climb(climb) => {
            let Some(target) = target_platform(state, &climb.to_platform) else {
                return false;
            };
            *state.pending_platform_id.borrow_mut() = Some(climb.to_platform.clone());
            state.overlay.climb(target.y, climb.meters).await;
            state
                .overlay
                .wait_for_climbed(STEP_TIMEOUT_S)
                .await
                .is_some()
        }
    }
}

/// The `{p.id: p}` lookup a HopDown/Climb step does against the latest
/// world snapshot: None when the target platform is gone.
fn target_platform<'a, O, H, G>(
    state: &'a AppState<O, H, G>,
    to_platform: &str,
) -> Option<&'a Platform>
where
    O: Overlay,
    H: StandUpHooks,
    G: Geometry,
{
    let platforms_by_id: HashMap<&str, &Platform> = state
        .world_platforms
        .iter()
        .map(|platform| (platform.id.as_str(), platform))
        .collect();
    platforms_by_id.get(to_platform).copied()
}
