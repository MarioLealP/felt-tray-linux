//! Minimal stand-ins for the other felt_animation modules nav_executor
//! drives - NOT ports of those modules (each has its own order/crate).
//! They exist so run_plan's real logic can be typed and tested here:
//!
//! - [`Platform`] - world.py's Platform (just the fields the ported code
//!   touches plus the ones the spec's fixtures construct).
//! - [`Walk`]/[`HopDown`]/[`Climb`]/[`Step`] - navigation.py's plan steps.
//! - [`Geometry`] - geometry.py's OverlayGeometry.px_to_world_x (the only
//!   geometry call nav_executor makes; the Win32 monitor queries in that
//!   module stay there).
//! - [`Overlay`] - the structural union of state.py's OverlayLike and the
//!   hop/climb/wait_for_landed/wait_for_climbed methods nav_executor calls
//!   directly on state.overlay (overlay.py's OverlayClient owns the real
//!   TCP relay + subprocess; tests pass a fake, same as the Python).
//! - [`LandedNotice`]/[`ClimbedNotice`] - main.gd's {"landed"}/{"climbed"}
//!   notice payloads as the Python fakes construct them.
//! - [`BehaviorState`] - state.py's BehaviorState, reduced to the surface
//!   this crate needs (posture, the base/upper activity locks, walk_body,
//!   run_sequence).
//! - [`stand_up_if_sitting`] + [`StandUpHooks`] - behavior.py's
//!   stand_up_if_sitting control flow; its two calls into other modules
//!   (ledge_sit.stand_up_from_ledge, props_director.stand_up_clip) go
//!   through the injectable [`StandUpHooks`] trait.
//! - [`AMBIENT_WALK_TAG`], [`CLIP_TIMEOUT`] - runtime.py constants.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::rc::Rc;
use std::sync::Mutex;
use std::task::{Poll, Waker};

// -- world.py ----------------------------------------------------------------

/// One stand-on-able ledge, desktop px. `id` is stable across ticks for the
/// same window/monitor/seam. `kind` is "floor" | "window" | "seam".
#[derive(Clone, Debug, PartialEq)]
pub struct Platform {
    pub id: String,
    pub hwnd: Option<i64>,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    pub kind: String,
}

// -- navigation.py -----------------------------------------------------------

/// Walk to an x on the platform she's currently on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walk {
    pub x_px: f64,
}

/// Step off, fall to a strictly lower platform.
#[derive(Clone, Debug, PartialEq)]
pub struct HopDown {
    pub to_platform: String,
    pub x_px: f64,
}

/// Root-motion-assisted step up onto a platform at most 2 m higher.
#[derive(Clone, Debug, PartialEq)]
pub struct Climb {
    pub to_platform: String,
    pub x_px: f64,
    pub meters: i32,
}

/// navigation.py's `Step = Walk | HopDown | Climb`.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Walk(Walk),
    HopDown(HopDown),
    Climb(Climb),
}

// -- geometry.py -------------------------------------------------------------

/// geometry.py's OverlayGeometry, reduced to the one pure conversion
/// nav_executor uses. (The module's ctypes/Win32 monitor enumeration stays
/// in that module's port; nothing here calls the OS.)
pub trait Geometry {
    /// Desktop px -> world x.
    fn px_to_world_x(&self, desktop_x: f64) -> f64;
}

// -- overlay.py --------------------------------------------------------------

/// main.gd's {"landed": {...}} notice - nav_executor only checks presence
/// vs timeout, but the payload rides along for other callers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandedNotice {
    pub height_px: f64,
}

/// main.gd's {"climbed": {...}} notice - same "presence vs timeout" use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClimbedNotice {
    pub y_px: f64,
}

/// The structural Protocol shape of overlay.py's OverlayClient that this
/// crate drives: state.py's OverlayLike plus the WP-23 slice-3 hop/climb
/// pair and their completion-notice waiters. The real implementation (a
/// subprocess + TCP relay to main.gd) lives in the overlay module's port;
/// tests pass a fake, exactly like the Python spec.
/// Note: `async fn` in this public trait is deliberate - the port is
/// single-threaded (Rc/RefCell state, no Send bounds needed anywhere).
#[allow(async_fn_in_trait)]
pub trait Overlay {
    async fn play_and_wait(&self, tag: &str, timeout: f64, clip: Option<&str>);
    async fn play_tag(&self, tag: &str, looping: bool, clip: Option<&str>);
    /// overlay.py's `move` (a Rust keyword here).
    async fn move_to(&self, x: f64, offscreen: bool, running: bool);
    /// Waits for main.gd's real {"arrived": true} notice - `timeout` is
    /// only a safety net; "no signal in time" is the caller's to decide.
    async fn wait_for_arrived(&self, timeout: f64);
    /// overlay.py's `send({"tag": tag})` - a raw send, no completion wait.
    async fn send_tag(&self, tag: &str) -> bool;
    /// {"hop": {"y_px": n}} - pair with [`Overlay::wait_for_landed`].
    async fn hop(&self, y_px: f64) -> bool;
    /// {"climb": {"y_px": n, "meters": 1|2}} - pair with
    /// [`Overlay::wait_for_climbed`].
    async fn climb(&self, y_px: f64, meters: i32) -> bool;
    /// The {"landed": {...}} payload once a fall (including a hop's) ends,
    /// or None on timeout / dropped connection.
    async fn wait_for_landed(&self, timeout: f64) -> Option<LandedNotice>;
    /// The {"climbed": {...}} payload once a climb ends, or None on
    /// timeout / dropped connection.
    async fn wait_for_climbed(&self, timeout: f64) -> Option<ClimbedNotice>;
    /// main.gd's {"layers"} report for `tag` (overlay.py's `tag_layer`).
    /// Absent -> layer_for(tag, {}) = "full" (the old single-lock
    /// behaviour).
    fn tag_layer(&self, _tag: &str) -> Option<&str> {
        None
    }
}

// -- runtime.py --------------------------------------------------------------

/// runtime.py's AMBIENT_WALK_TAG - the walk cycle tag, when known.
pub const AMBIENT_WALK_TAG: &str = "locomotion/walk/normal_walk";

/// runtime.py's CLIP_TIMEOUT - the safety-net timeout behavior.py's
/// stand_up path passes for its one-shot clips.
pub const CLIP_TIMEOUT: f64 = 60.0;

// -- state.py ----------------------------------------------------------------

/// state.py's posture field: "standing" or "sitting".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    Standing,
    Sitting,
}

#[derive(Default)]
struct LockState {
    locked: bool,
    wakers: Vec<Waker>,
}

/// Minimal asyncio.Lock shape: not reentrant - holding it and taking it
/// again deadlocks, same as the Python. A released lock wakes all waiters
/// (notify-all on a flag), so a dropped/cancelled waiter cannot strand the
/// one behind it.
pub struct AsyncLock {
    state: Mutex<LockState>,
}

impl AsyncLock {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LockState::default()),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.state.lock().unwrap().locked
    }

    pub async fn lock(&self) -> AsyncLockGuard<'_> {
        poll_fn(|cx| {
            let mut state = self.state.lock().unwrap();
            if !state.locked {
                state.locked = true;
                return Poll::Ready(());
            }
            state.wakers.push(cx.waker().clone());
            Poll::Pending
        })
        .await;
        AsyncLockGuard { lock: self }
    }
}

impl Default for AsyncLock {
    fn default() -> Self {
        Self::new()
    }
}

/// Held while an [`AsyncLock`] is held; releases on drop.
pub struct AsyncLockGuard<'a> {
    lock: &'a AsyncLock,
}

impl Drop for AsyncLockGuard<'_> {
    fn drop(&mut self) {
        let wakers = {
            let mut state = self.lock.state.lock().unwrap();
            state.locked = false;
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

/// state.py's BehaviorState, reduced to what nav_executor's plan path and
/// its stand-up step need: the base/upper activity locks (WP-20), the
/// posture field (WP-21), and the lock-free walk_body/run_sequence bodies.
pub struct BehaviorState<O: Overlay> {
    overlay: Rc<O>,
    posture: Cell<Posture>,
    base_lock: AsyncLock,
    upper_lock: AsyncLock,
}

impl<O: Overlay> BehaviorState<O> {
    pub fn new(overlay: Rc<O>) -> Self {
        Self {
            overlay,
            posture: Cell::new(Posture::Standing),
            base_lock: AsyncLock::new(),
            upper_lock: AsyncLock::new(),
        }
    }

    pub fn posture(&self) -> Posture {
        self.posture.get()
    }

    pub fn set_posture(&self, posture: Posture) {
        self.posture.set(posture);
    }

    /// Anything at all is playing (either layer).
    pub fn busy(&self) -> bool {
        self.base_lock.is_locked() || self.upper_lock.is_locked()
    }

    pub fn base_busy(&self) -> bool {
        self.base_lock.is_locked()
    }

    pub fn upper_busy(&self) -> bool {
        self.upper_lock.is_locked()
    }

    /// The class main.gd reported for `tag`; unknown -> "full".
    pub fn layer_of(&self, tag: &str) -> &str {
        self.overlay.tag_layer(tag).unwrap_or("full")
    }

    pub fn is_upper(&self, tag: &str) -> bool {
        self.layer_of(tag) == "upper"
    }

    /// WP-21: a multi-step sequence holds the BASE lock throughout. Always
    /// base before upper (see run_sequence), so two sequences can't
    /// deadlock.
    pub async fn hold_base(&self) -> AsyncLockGuard<'_> {
        self.base_lock.lock().await
    }

    pub async fn hold_upper(&self) -> AsyncLockGuard<'_> {
        self.upper_lock.lock().await
    }

    /// walk_to without taking the lock - for a caller already holding the
    /// base lock (WP-21 prop sequences / nav_executor's run_plan).
    // state.py's walk_body has this exact 7-parameter shape; keeping it is
    // the point of the shared type.
    #[allow(clippy::too_many_arguments)]
    pub async fn walk_body(
        &self,
        target_x: f64,
        cycle_tag: Option<&str>,
        running: bool,
        start_tag: Option<&str>,
        stop_tag: Option<&str>,
        offscreen: bool,
        clip_timeout: f64,
    ) {
        if let Some(start_tag) = start_tag {
            self.overlay
                .play_and_wait(start_tag, clip_timeout, None)
                .await;
        }
        // The arrived waiter is armed (first-polled) before the move goes
        // out, so an instant reply (a move to where she already stands)
        // can't be missed. Dropping this join on cancellation drops the
        // waiter with it - no orphaned waiter left behind.
        let walk = async {
            self.overlay.move_to(target_x, offscreen, running).await;
            if let Some(cycle_tag) = cycle_tag {
                self.overlay.play_tag(cycle_tag, true, None).await;
            }
        };
        join2(self.overlay.wait_for_arrived(clip_timeout), walk).await;
        if let Some(stop_tag) = stop_tag {
            self.overlay
                .play_and_wait(stop_tag, clip_timeout, None)
                .await;
        }
    }

    /// Runs a one-shot sequence of tags back to back, holding the lock(s)
    /// its tags need for the whole thing. An all-upper-body sequence takes
    /// only the upper lock and plays over a walk or a sit; anything with a
    /// base/full-body tag takes both (base first, then upper - always that
    /// order).
    pub async fn run_sequence(
        &self,
        tags: &[&str],
        clip_timeout: f64,
        clips: Option<&HashMap<String, String>>,
    ) {
        let all_upper = !tags.is_empty() && tags.iter().all(|tag| self.is_upper(tag));
        let _base = if all_upper {
            None
        } else {
            Some(self.base_lock.lock().await)
        };
        let _upper = self.upper_lock.lock().await;
        for tag in tags {
            let clip = clips.and_then(|clips| clips.get(*tag));
            self.overlay
                .play_and_wait(tag, clip_timeout, clip.map(String::as_str))
                .await;
        }
    }
}

/// Minimal two-future join so the arrived waiter can run alongside the
/// move without pulling in a futures crate. Polls `a` first on every wake,
/// so `a` (the waiter) is always armed before `b` (the move) is sent.
async fn join2<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let mut a_out = None;
    let mut b_out = None;
    poll_fn(move |cx| {
        if a_out.is_none()
            && let Poll::Ready(out) = a.as_mut().poll(cx)
        {
            a_out = Some(out);
        }
        if b_out.is_none()
            && let Poll::Ready(out) = b.as_mut().poll(cx)
        {
            b_out = Some(out);
        }
        match (a_out.take(), b_out.take()) {
            (Some(av), Some(bv)) => Poll::Ready((av, bv)),
            (av, bv) => {
                // put back whichever side (if any) already finished
                a_out = av;
                b_out = bv;
                Poll::Pending
            }
        }
    })
    .await
}

// -- behavior.py -------------------------------------------------------------

/// behavior.py's stand_up_if_sitting: the "walk_to stands her up first"
/// path - clear a ledge sit first (a different posture mechanism than the
/// WP-21 floor chair), then the chair posture. The two calls that live in
/// modules not ported here go through [`StandUpHooks`].
pub async fn stand_up_if_sitting<O: Overlay, H: StandUpHooks, G: Geometry>(
    state: &AppState<O, H, G>,
) {
    // stand_up_from_ledge clears the flag at its top before any await, so
    // taking it here first is behaviour-identical.
    if state.seated_on_ledge.borrow_mut().take().is_some() {
        state.hooks.stand_up_from_ledge().await;
    }
    if state.behavior.posture() != Posture::Sitting {
        return;
    }
    if state.known_tags.contains("transition/sit_to_stand") {
        // sit_to_stand_clips: the transition clip that fits the chair.
        let clips = state
            .hooks
            .sit_to_stand_clip()
            .map(|clip| HashMap::from([("transition/sit_to_stand".to_string(), clip)]));
        state
            .behavior
            .run_sequence(&["transition/sit_to_stand"], CLIP_TIMEOUT, clips.as_ref())
            .await;
    }
    state.behavior.set_posture(Posture::Standing);
    // Otherwise idle/sitting can flash between the stand-up and the walk.
    if state.known_tags.contains("idle") {
        state.overlay.send_tag("idle").await;
    }
}

/// The parts of stand_up_if_sitting that live in modules not ported in
/// this crate: ledge_sit.stand_up_from_ledge (the ledge stand-up overlay
/// exchange plus stay-timer cancel) and behavior.sit_to_stand_clips'
/// props_director stand_up_clip lookup. Injected on [`AppState`]; tests
/// use [`NoopStandUpHooks`] (the spec never sits).
/// Note: `async fn` in this public trait is deliberate - same single-
/// threaded shape as [`Overlay`].
#[allow(async_fn_in_trait)]
pub trait StandUpHooks {
    /// ledge_sit.py's stand_up_from_ledge, minus the seated_on_ledge
    /// clear (the caller does that first, like the Python does).
    async fn stand_up_from_ledge(&self);
    /// props_director.stand_up_clip() - the exact clip for the chair.
    fn sit_to_stand_clip(&self) -> Option<String>;
}

/// [`StandUpHooks`] that does nothing - for callers that never sit.
pub struct NoopStandUpHooks;

impl StandUpHooks for NoopStandUpHooks {
    async fn stand_up_from_ledge(&self) {}

    fn sit_to_stand_clip(&self) -> Option<String> {
        None
    }
}

// -- runtime.py's AppState, reduced to the fields this crate touches ----------

/// The duck-typed slice of runtime.py's AppState that nav_executor and its
/// stand-up step read/mutate. `current_x`/`pending_platform_id`/
/// `seated_on_ledge` use interior mutability so `run_plan` can take the
/// state by shared reference while the base lock is held against it.
pub struct AppState<O: Overlay, H: StandUpHooks, G: Geometry> {
    pub overlay: Rc<O>,
    pub behavior: BehaviorState<O>,
    pub hooks: H,
    pub geometry: G,
    pub known_tags: HashSet<String>,
    pub current_x: Cell<f64>,
    pub world_platforms: Vec<Platform>,
    pub pending_platform_id: RefCell<Option<String>>,
    pub seated_on_ledge: RefCell<Option<String>>,
}

impl<O: Overlay, H: StandUpHooks, G: Geometry> AppState<O, H, G> {
    /// The state's defaults match the Python spec's SimpleNamespace fake:
    /// known_tags empty, current_x 0.0, pending_platform_id None,
    /// seated_on_ledge None.
    pub fn new(overlay: Rc<O>, hooks: H, geometry: G, world_platforms: Vec<Platform>) -> Self {
        Self {
            behavior: BehaviorState::new(Rc::clone(&overlay)),
            overlay,
            hooks,
            geometry,
            known_tags: HashSet::new(),
            current_x: Cell::new(0.0),
            world_platforms,
            pending_platform_id: RefCell::new(None),
            seated_on_ledge: RefCell::new(None),
        }
    }
}
