//! Minimal stand-ins for the felt_animation modules ledge_sit.py leans
//! on (`world.Platform`, `state.BehaviorState`, `overlay.OverlayClient`'s
//! message surface, `runtime` bookkeeping). Only what ledge_sit and its
//! tests need lives here; the real modules own the rest.

use std::cell::Cell;
use std::collections::HashMap;

use crate::exec::{AsyncMutex, AsyncMutexGuard, BoxFuture};

/// `mood.as_dict()` - a mood snapshot: axis -> value.
pub type Mood = HashMap<String, f64>;

/// `world.Platform.kind` - "floor" | "window" | "seam".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Window,
    Seam,
    Floor,
}

/// `world.Platform` - one stand-on-able ledge, desktop px.
#[derive(Debug, Clone, PartialEq)]
pub struct Platform {
    pub id: String,
    pub hwnd: Option<i64>,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    pub kind: PlatformKind,
}

/// `world.Monitor` - one monitor's bounds, desktop px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub index: i32,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub work_bottom: Option<f64>,
}

/// Injectable randomness (Python's `random` module / `random.Random`).
pub trait RandomSource {
    /// `random.random()` - uniform in [0.0, 1.0).
    fn next_unit(&mut self) -> f64;

    /// `random.uniform(lo, hi)` - `lo + (hi - lo) * random()`.
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_unit()
    }
}

/// Injectable clock (Python's `asyncio.sleep`).
pub trait Clock {
    /// Resolves after `secs` of this clock's time.
    fn sleep(&self, secs: f64) -> BoxFuture<'_, ()>;
}

/// A stay-then-stand timer task - the `asyncio.Task` duck type behind
/// `state.ledge_sit_task` (real tasks and tests' `_FakeTask` both
/// satisfy it).
pub trait SitTask {
    fn is_done(&self) -> bool;
    fn cancel(&self);
    /// `task is asyncio.current_task()`.
    fn is_current(&self) -> bool;
    /// `await task`.
    fn wait(&self) -> BoxFuture<'_, ()>;
}

/// One message on the overlay wire - the dict payloads
/// `OverlayClient.ledge_sit/ledge_stand/move` send (this crate is
/// transport-free, so the payload is typed instead of JSON).
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayMsg {
    /// `{"ledge_sit": {"y_px": n}}`
    LedgeSit { y_px: f64 },
    /// `{"ledge_stand": {}}`
    LedgeStand,
    /// `{"move": {"x": n, ...}}`
    Move {
        x: f64,
        offscreen: bool,
        running: bool,
    },
}

/// The slice of `overlay.OverlayClient` ledge_sit and its behavior
/// stand-in use.
pub trait Overlay {
    /// `OverlayClient.send` - writes one message; False when the overlay
    /// is not connected.
    fn send(&self, msg: OverlayMsg) -> BoxFuture<'_, bool>;

    /// `OverlayClient.wait_for_seated` - the next `{"seated": true|false}`
    /// value, or None on timeout / dropped connection.
    fn wait_for_seated(&self, timeout_s: f64) -> BoxFuture<'_, Option<bool>>;

    /// `OverlayClient.ledge_sit`.
    fn ledge_sit(&self, y_px: f64) -> BoxFuture<'_, bool> {
        Box::pin(async move { self.send(OverlayMsg::LedgeSit { y_px }).await })
    }

    /// `OverlayClient.ledge_stand`.
    fn ledge_stand(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move { self.send(OverlayMsg::LedgeStand).await })
    }

    /// `OverlayClient.move` - the method name `move` is a Rust keyword.
    fn move_to(&self, x: f64, offscreen: bool, running: bool) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.send(OverlayMsg::Move {
                x,
                offscreen,
                running,
            })
            .await;
        })
    }
}

/// `runtime.AppState.config`'s event-reporting settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventConfig {
    /// Empty disables reporting ("empty = skipped", like behavior.react_event).
    pub felt_core_url: String,
    pub felt_event_reporting_enabled: bool,
}

/// The network side of `event_log`'s fire-and-forget POST to felt-core -
/// a trait with no implementation in this pure-logic crate.
pub trait EventSink {
    fn post_event(&self, kind: &str);
}

/// `state.BehaviorState.posture` - starts standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Posture {
    #[default]
    Standing,
    Sitting,
}

/// `state.BehaviorState` reduced to what ledge_sit touches: the base
/// lock (`hold_base`) and the posture word.
#[derive(Default)]
pub struct BehaviorState {
    base: AsyncMutex,
    posture: Cell<Posture>,
}

impl BehaviorState {
    /// `BehaviorState.base_busy`.
    pub fn base_busy(&self) -> bool {
        self.base.is_locked()
    }

    /// `async with behavior.hold_base():`.
    pub fn hold_base(&self) -> BoxFuture<'_, AsyncMutexGuard<'_>> {
        self.base.lock()
    }

    pub fn posture(&self) -> Posture {
        self.posture.get()
    }

    pub fn set_posture(&self, posture: Posture) {
        self.posture.set(posture);
    }
}
