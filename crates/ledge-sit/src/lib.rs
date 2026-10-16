//! WP-23 slice 5: sitting on a window/seam ledge.
//!
//! Mario's own decision (2026-09-26): no dedicated ledge-sit clip - reuse
//! the existing chair-sit animations. main.gd (WP-23 build notes, slice 5)
//! plays `sit/chair_start` -> `sit/chair_loop` (or `idle/sitting` as a
//! fallback) with her hips pinned to the ledge's own y, so her legs hang
//! over the edge; this module only decides WHEN that happens and for how
//! long (Godot is presentation only, ENGINEERING-RULES.md rule 32).
//!
//! Kept as its own module rather than growing loops.py (rule 1/2): the
//! sit/stand decisions, the mood-weighted chance and the stay-then-stand
//! timer are one job, separate from the world tick and the ambient wander
//! that call into it.
//!
//! Randomness and time are injectable ([`shared::RandomSource`],
//! [`shared::Clock`]) so tests are deterministic, replacing the Python
//! tests' `monkeypatch` of `random.random` / `asyncio.sleep`.

pub mod behavior;
pub mod debug;
pub mod event_log;
pub mod exec;
pub mod overlay;
pub mod runtime;
pub mod shared;

use std::rc::Rc;

use runtime::AppState;
use shared::{Mood, Platform, PlatformKind, RandomSource, SitTask};

/// `wait_for_seated`'s safety-net timeout (seconds).
pub const SEATED_ACK_TIMEOUT_S: f64 = 5.0;

/// How long she stays seated before standing up on her own (the WP's own
/// "constant range" wording), seconds - shortened when bored (she'd rather
/// get up and go look at something than linger).
pub const SIT_DURATION_MIN_S: f64 = 45.0;
pub const SIT_DURATION_MAX_S: f64 = 180.0;
pub const BORED_DURATION_SCALE: f64 = 0.6;
pub const BORED_DURATION_THRESHOLD: f64 = 0.6;

/// The chance she sits down after an ORDINARY ambient arrival on a ledge
/// (never the floor - see [`maybe_sit_after_arrival`]'s kind check). The
/// explicit Ctrl+Alt+G ledge action always sits (`force = true`)
/// regardless of this - Mario's own instruction.
pub const BASE_SIT_CHANCE: f64 = 0.35;
pub const SLEEPY_ENERGY_THRESHOLD: f64 = 0.35;
pub const SLEEPY_SIT_CHANCE: f64 = 0.85;
pub const BORED_SIT_CHANCE_PENALTY: f64 = 0.2;

/// Mood-weighted chance she sits down on a ledge she just arrived on
/// (ambient wander only - the explicit ledge action always sits). Low
/// energy ("sleepy") makes it likely; boredom makes it less likely (she'd
/// rather explore than settle down).
pub fn sit_chance(mood: &Mood) -> f64 {
    let energy = mood.get("energy").copied().unwrap_or(0.5);
    let boredom = mood.get("boredom").copied().unwrap_or(0.0);
    let mut chance = BASE_SIT_CHANCE;
    if energy < SLEEPY_ENERGY_THRESHOLD {
        let sleepy = (SLEEPY_ENERGY_THRESHOLD - energy) / SLEEPY_ENERGY_THRESHOLD;
        chance += sleepy * (SLEEPY_SIT_CHANCE - BASE_SIT_CHANCE);
    }
    chance -= boredom * BORED_SIT_CHANCE_PENALTY;
    // max(0.0, min(1.0, chance)) - clamped to a probability.
    chance.clamp(0.0, 1.0)
}

/// A constant-range stay (45-180s per the WP), shorter when bored.
/// `rng` is the injectable stand-in for `random.Random` (Python's
/// optional `rng` argument falling back to the module-global `random`).
pub fn sit_duration_s(mood: &Mood, rng: &mut dyn RandomSource) -> f64 {
    let mut lo = SIT_DURATION_MIN_S;
    let mut hi = SIT_DURATION_MAX_S;
    if mood.get("boredom").copied().unwrap_or(0.0) >= BORED_DURATION_THRESHOLD {
        lo *= BORED_DURATION_SCALE;
        hi *= BORED_DURATION_SCALE;
    }
    rng.uniform(lo, hi)
}

/// Sits on `platform` - a window/seam ledge she's already standing on.
/// Caller must NOT hold the base lock (this takes it briefly for the
/// sit-down transition, same convention as props.PropDirector.sit_down).
/// Schedules the stay-then-stand-up timer. Returns false if the overlay
/// rejected the message (not connected).
pub async fn sit_on_ledge(state: &Rc<AppState>, platform: &Platform) -> bool {
    {
        let _guard = state.behavior.hold_base().await;
        if !state.overlay.ledge_sit(platform.y).await {
            return false;
        }
        state.overlay.wait_for_seated(SEATED_ACK_TIMEOUT_S).await;
    }
    *state.seated_on_ledge.borrow_mut() = Some(platform.id.clone());
    // logging: "ledge: sitting on {id}"
    event_log::report_event(state, "ledge_sit");
    let task: Rc<dyn SitTask> = exec::spawn({
        let state = Rc::clone(state);
        async move { sit_then_stand(state).await }
    });
    *state.ledge_sit_task.borrow_mut() = Some(task);
    true
}

/// The stay-then-stand timer body (`_sit_then_stand`): wait out the stay
/// duration, then stand up on her own. Cancellation drops the future,
/// which is asyncio's CancelledError propagation.
async fn sit_then_stand(state: Rc<AppState>) {
    let secs = {
        let mut rng = state.rng.borrow_mut();
        sit_duration_s(&state.mood, &mut **rng)
    };
    state.clock.sleep(secs).await;
    stand_up_from_ledge(&state).await;
}

/// Stands her up off the ledge (sit/chair_end) if she's seated on one -
/// a no-op otherwise. Cancels the stay timer if it's still pending (this
/// IS that timer firing early, or something else - a move, WP-38's own
/// `stand_up_if_sitting` path - stood her up first).
pub async fn stand_up_from_ledge(state: &Rc<AppState>) {
    {
        let mut seated = state.seated_on_ledge.borrow_mut();
        if seated.is_none() {
            return;
        }
        *seated = None;
    }
    let task = state.ledge_sit_task.borrow_mut().take();
    if let Some(task) = &task
        && !task.is_current()
        && !task.is_done()
    {
        task.cancel();
    }
    let _guard = state.behavior.hold_base().await;
    state.overlay.ledge_stand().await;
    state.overlay.wait_for_seated(SEATED_ACK_TIMEOUT_S).await;
    // logging: "ledge: stood up"
}

/// Called once a navigation plan has landed her on `platform`. Never
/// sits on a floor. `force = true` (the explicit Ctrl+Alt+G ledge
/// action) always sits; otherwise a mood-weighted chance ([`sit_chance`]) -
/// "always sit when she got there via the explicit ledge action" (the WP's
/// own wording).
pub async fn maybe_sit_after_arrival(
    state: &Rc<AppState>,
    platform: &Platform,
    mood: &Mood,
    force: bool,
) -> bool {
    if platform.kind == PlatformKind::Floor {
        return false;
    }
    if !force {
        let roll = state.rng.borrow_mut().next_unit();
        if roll >= sit_chance(mood) {
            return false;
        }
    }
    sit_on_ledge(state, platform).await
}
