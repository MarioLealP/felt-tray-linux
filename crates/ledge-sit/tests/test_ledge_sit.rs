//! WP-23 slice 5: ledge_sit's pure logic (mood-weighted sit chance/
//! duration, never sitting on a floor) plus its async orchestrators
//! against fakes, following test_platform_tracking.py/
//! test_window_events.py's patterns (a fake overlay, stand-in app state).
//!
//! Port of ref/felt-animation/tests/test_ledge_sit.py. The Python
//! tests' `monkeypatch` of `random.random` / `asyncio.sleep` is replaced
//! by the crate's injectable [`RandomSource`] / [`Clock`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use felt_ledge_sit::behavior::{stand_up_if_sitting, walk_to};
use felt_ledge_sit::debug::{WorldState, world_endpoint};
use felt_ledge_sit::exec::{BoxFuture, block_on};
use felt_ledge_sit::overlay::OverlayClient;
use felt_ledge_sit::runtime::{AppState, WorldMsg};
use felt_ledge_sit::shared::{
    Clock, EventConfig, EventSink, Mood, Overlay, OverlayMsg, Platform, PlatformKind, RandomSource,
    SitTask,
};
use felt_ledge_sit::{
    BORED_DURATION_SCALE, SIT_DURATION_MAX_S, SIT_DURATION_MIN_S, maybe_sit_after_arrival,
    sit_chance, sit_duration_s, sit_on_ledge, stand_up_from_ledge,
};

fn ledge() -> Platform {
    Platform {
        id: "win:1:0".to_string(),
        hwnd: Some(1),
        x0: 100.0,
        x1: 900.0,
        y: 600.0,
        kind: PlatformKind::Window,
    }
}

fn floor() -> Platform {
    Platform {
        id: "floor:0".to_string(),
        hwnd: None,
        x0: 0.0,
        x1: 1920.0,
        y: 1040.0,
        kind: PlatformKind::Floor,
    }
}

fn neutral_mood() -> Mood {
    Mood::from([
        ("energy".to_string(), 0.5),
        ("boredom".to_string(), 0.0),
        ("happiness".to_string(), 0.5),
        ("affection".to_string(), 0.5),
        ("annoyance".to_string(), 0.0),
    ])
}

fn sleepy_mood() -> Mood {
    let mut mood = neutral_mood();
    mood.insert("energy".to_string(), 0.1);
    mood
}

fn bored_mood() -> Mood {
    let mut mood = neutral_mood();
    mood.insert("boredom".to_string(), 0.9);
    mood
}

// -- fakes ---------------------------------------------------------------------

/// `_FakeOverlay` - logs the payloads a real OverlayClient would send.
struct FakeOverlay {
    sent: RefCell<Vec<OverlayMsg>>,
    seated_ack: Option<bool>,
}

impl Default for FakeOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeOverlay {
    /// `_FakeOverlay(seated_ack=True)` by default.
    fn new() -> Self {
        Self {
            sent: RefCell::new(Vec::new()),
            seated_ack: Some(true),
        }
    }
}

impl Overlay for FakeOverlay {
    fn send(&self, msg: OverlayMsg) -> BoxFuture<'_, bool> {
        self.sent.borrow_mut().push(msg);
        Box::pin(std::future::ready(true))
    }

    fn wait_for_seated(&self, _timeout_s: f64) -> BoxFuture<'_, Option<bool>> {
        Box::pin(std::future::ready(self.seated_ack))
    }
}

/// A clock whose sleep never elapses - the stay timer stands her up only
/// when a test asks it to (Python's real 45-180s `asyncio.sleep` never
/// fired inside a test either).
struct NeverSleeper;

impl Clock for NeverSleeper {
    fn sleep(&self, _secs: f64) -> BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

/// `_fast_sleep` stand-in: records the requested seconds and returns
/// immediately.
#[derive(Default)]
struct RecordingSleeper {
    slept: RefCell<Vec<f64>>,
}

impl Clock for RecordingSleeper {
    fn sleep(&self, secs: f64) -> BoxFuture<'_, ()> {
        self.slept.borrow_mut().push(secs);
        Box::pin(std::future::ready(()))
    }
}

/// A `random.random()` always returning the same value (the tests'
/// `monkeypatch.setattr(ledge_sit.random, "random", lambda: 0.999999)`).
struct FixedRng(f64);

impl RandomSource for FixedRng {
    fn next_unit(&mut self) -> f64 {
        self.0
    }
}

/// Draws the uniform range's endpoints alternately, so the range's edge
/// cases are what the duration assertions see.
#[derive(Default)]
struct AlternatingRng {
    flip: bool,
}

impl RandomSource for AlternatingRng {
    fn next_unit(&mut self) -> f64 {
        self.flip = !self.flip;
        if self.flip { 1.0 } else { 0.0 }
    }
}

/// Never called in these tests (the config's empty `felt_core_url`
/// makes report_event a no-op, like WP-26 intended).
struct NoopSink;

impl EventSink for NoopSink {
    fn post_event(&self, _kind: &str) {}
}

/// `_FakeTask` - a cancel()/done()/cancelled() stand-in that avoids a
/// real spawned task in the plain (non-async) tests.
#[derive(Default)]
struct FakeTask {
    cancelled: Cell<bool>,
}

impl FakeTask {
    /// `_FakeTask.cancelled()`.
    fn cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

impl SitTask for FakeTask {
    fn is_done(&self) -> bool {
        false // _FakeTask.done() is always False
    }

    fn cancel(&self) {
        self.cancelled.set(true);
    }

    fn is_current(&self) -> bool {
        false
    }

    fn wait(&self) -> BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

/// `_make_state` - the AppState fields ledge_sit touches. The empty
/// `felt_core_url` makes report_event("ledge_sit") a no-op - no network
/// access needed here (WP-26).
fn make_state(
    overlay: Rc<FakeOverlay>,
    mood: Mood,
    clock: Rc<dyn Clock>,
    rng: Box<dyn RandomSource>,
) -> Rc<AppState> {
    Rc::new(AppState::new(
        overlay,
        mood,
        EventConfig {
            felt_core_url: String::new(),
            felt_event_reporting_enabled: true,
        },
        clock,
        rng,
        Rc::new(NoopSink),
    ))
}

/// `_make_state` with the default test injectables: a stay timer that
/// never fires and a mid-range duration draw.
fn quiet_state(overlay: Rc<FakeOverlay>, mood: Mood) -> Rc<AppState> {
    make_state(
        overlay,
        mood,
        Rc::new(NeverSleeper),
        Box::new(FixedRng(0.5)),
    )
}

// -- sit_chance / sit_duration_s (pure) ---------------------------------------

#[test]
fn test_sit_chance_higher_when_sleepy() {
    assert!(sit_chance(&sleepy_mood()) > sit_chance(&neutral_mood()));
}

#[test]
fn test_sit_chance_lower_when_bored() {
    assert!(sit_chance(&bored_mood()) < sit_chance(&neutral_mood()));
}

#[test]
fn test_sit_chance_is_a_probability() {
    for mood in [neutral_mood(), sleepy_mood(), bored_mood(), Mood::new()] {
        assert!((0.0..=1.0).contains(&sit_chance(&mood)));
    }
}

#[test]
fn test_sit_duration_within_the_constant_range() {
    let mut rng = AlternatingRng::default();
    for _ in 0..50 {
        let d = sit_duration_s(&neutral_mood(), &mut rng);
        assert!((SIT_DURATION_MIN_S..=SIT_DURATION_MAX_S).contains(&d));
    }
}

#[test]
fn test_sit_duration_shorter_when_bored() {
    let mut rng = AlternatingRng::default();
    let mut bored_max = f64::NEG_INFINITY;
    for _ in 0..50 {
        bored_max = bored_max.max(sit_duration_s(&bored_mood(), &mut rng));
    }
    assert!(bored_max <= SIT_DURATION_MAX_S * BORED_DURATION_SCALE + 1e-9);
}

// -- maybe_sit_after_arrival (async, pure decision + the sit call) --------------

#[test]
fn test_never_sits_on_a_floor_even_when_forced() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = quiet_state(Rc::clone(&overlay), neutral_mood());
        let sat = maybe_sit_after_arrival(&state, &floor(), &neutral_mood(), true).await;
        assert!(!sat);
        assert!(state.seated_on_ledge.borrow().is_none());
        assert!(overlay.sent.borrow().is_empty());
    });
}

#[test]
fn test_explicit_ledge_action_always_sits_regardless_of_chance() {
    block_on(async {
        // A roll of 0.999999 would fail almost any real chance roll -
        // force=true must sit anyway (Mario's own "always sit when she
        // got there via the explicit ledge action").
        let overlay = Rc::new(FakeOverlay::new());
        let state = make_state(
            Rc::clone(&overlay),
            neutral_mood(),
            Rc::new(NeverSleeper),
            Box::new(FixedRng(0.999999)),
        );
        let sat = maybe_sit_after_arrival(&state, &ledge(), &neutral_mood(), true).await;
        assert!(sat);
        assert_eq!(state.seated_on_ledge.borrow().as_deref(), Some("win:1:0"));
        assert!(
            overlay
                .sent
                .borrow()
                .contains(&OverlayMsg::LedgeSit { y_px: 600.0 })
        );
        let task = state.ledge_sit_task.borrow().clone().expect("a stay timer");
        task.cancel();
        task.wait().await;
    });
}

#[test]
fn test_ambient_arrival_respects_the_mood_chance() {
    block_on(async {
        // 0.999999 fails almost any chance roll.
        let overlay = Rc::new(FakeOverlay::new());
        let state = make_state(
            overlay,
            neutral_mood(),
            Rc::new(NeverSleeper),
            Box::new(FixedRng(0.999999)),
        );
        let sat = maybe_sit_after_arrival(&state, &ledge(), &neutral_mood(), false).await;
        assert!(!sat);
        assert!(state.seated_on_ledge.borrow().is_none());
    });
}

// -- sit_on_ledge / stand_up_from_ledge (async orchestrators) -------------------

#[test]
fn test_sit_on_ledge_sends_ledge_sit_and_tracks_state() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = quiet_state(Rc::clone(&overlay), neutral_mood());
        let ok = sit_on_ledge(&state, &ledge()).await;
        assert!(ok);
        assert_eq!(state.seated_on_ledge.borrow().as_deref(), Some("win:1:0"));
        assert_eq!(
            *overlay.sent.borrow(),
            vec![OverlayMsg::LedgeSit { y_px: 600.0 }]
        );
        assert!(!state.behavior.base_busy()); // released once the sit-down finished
        let task = state.ledge_sit_task.borrow().clone().expect("a stay timer");
        task.cancel();
        task.wait().await;
    });
}

#[test]
fn test_stand_up_from_ledge_sends_ledge_stand_and_clears_state() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = quiet_state(Rc::clone(&overlay), neutral_mood());
        sit_on_ledge(&state, &ledge()).await;
        overlay.sent.borrow_mut().clear();
        let timer_task = state.ledge_sit_task.borrow().clone().expect("a stay timer");

        stand_up_from_ledge(&state).await;

        assert!(state.seated_on_ledge.borrow().is_none());
        assert!(state.ledge_sit_task.borrow().is_none());
        assert_eq!(*overlay.sent.borrow(), vec![OverlayMsg::LedgeStand]);
        timer_task.wait().await;
    });
}

#[test]
fn test_stand_up_from_ledge_is_a_no_op_when_not_seated() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = quiet_state(Rc::clone(&overlay), neutral_mood());
        stand_up_from_ledge(&state).await;
        assert!(overlay.sent.borrow().is_empty());
    });
}

#[test]
fn test_seated_timeout_stands_her_up() {
    // "Stay seated a while, then stand and resume roaming" - the timer
    // (sit_then_stand) fires stand_up_from_ledge once its sleep elapses.
    block_on(async {
        let clock = Rc::new(RecordingSleeper::default());
        let clock_slot: Rc<dyn Clock> = Rc::<RecordingSleeper>::clone(&clock);
        let overlay = Rc::new(FakeOverlay::new());
        let state = make_state(
            Rc::clone(&overlay),
            neutral_mood(),
            clock_slot,
            Box::new(FixedRng(0.5)),
        );
        sit_on_ledge(&state, &ledge()).await;
        let task = state.ledge_sit_task.borrow().clone().expect("a stay timer");
        task.wait().await; // the fast-sleep timer runs to completion immediately

        let slept = clock.slept.borrow();
        assert!(!slept.is_empty());
        assert!((SIT_DURATION_MIN_S..=SIT_DURATION_MAX_S).contains(&slept[0]));
        drop(slept);
        assert!(state.seated_on_ledge.borrow().is_none());
        assert!(overlay.sent.borrow().contains(&OverlayMsg::LedgeStand));
    });
}

// -- stand_up_if_sitting covers seated-on-ledge (behavior.py) -------------------

#[test]
fn test_seated_move_stands_first_then_walks() {
    // "An incoming move stands her up first (chair_end) then walks" -
    // behavior.walk_to always runs stand_up_if_sitting before the move,
    // and stand_up_if_sitting checks seated_on_ledge before the WP-21
    // chair posture (the same path walk_to already "stands her up first"
    // through).
    block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = quiet_state(Rc::clone(&overlay), neutral_mood());
        sit_on_ledge(&state, &ledge()).await;
        overlay.sent.borrow_mut().clear();

        walk_to(&state, 5.0, "locomotion/walk/normal_walk", false).await;

        assert!(state.seated_on_ledge.borrow().is_none());
        let sent = overlay.sent.borrow();
        let stand_index = sent
            .iter()
            .position(|m| *m == OverlayMsg::LedgeStand)
            .expect("a stand-up");
        let move_index = sent
            .iter()
            .position(|m| matches!(m, OverlayMsg::Move { .. }))
            .expect("a move");
        assert!(stand_index < move_index);
    });
}

#[test]
fn test_stand_up_if_sitting_is_a_no_op_when_standing() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = quiet_state(Rc::clone(&overlay), neutral_mood());
        stand_up_if_sitting(&state).await;
        assert!(overlay.sent.borrow().is_empty());
    });
}

// -- support drop while seated: falls without chair_end (runtime.py) -----------

#[test]
fn test_support_drop_while_seated_unseats_without_sending_chair_end() {
    // main.gd itself decides "stand instantly, no chair_end, fall as
    // usual" the moment a support drop arrives while seated - felt-animation
    // never sends ledge_stand for this case, it only syncs its OWN
    // bookkeeping off the resulting unprompted {"seated": false} notice
    // (AppState._on_world_settled, an on_message callback).
    let overlay = Rc::new(FakeOverlay::new());
    let state = quiet_state(overlay, neutral_mood());
    let task = Rc::new(FakeTask::default());
    let task_slot: Rc<dyn SitTask> = Rc::<FakeTask>::clone(&task);
    *state.seated_on_ledge.borrow_mut() = Some("win:1:0".to_string());
    *state.ledge_sit_task.borrow_mut() = Some(task_slot);

    state.on_world_settled(WorldMsg::Seated(false));

    assert!(state.seated_on_ledge.borrow().is_none());
    assert!(state.ledge_sit_task.borrow().is_none());
    assert!(task.cancelled());
}

#[test]
fn test_own_deliberate_stand_up_does_not_get_reprocessed() {
    // stand_up_from_ledge already clears seated_on_ledge before its own
    // ledge_stand ack arrives, so _on_world_settled must not act a second
    // time on that same notice.
    let overlay = Rc::new(FakeOverlay::new());
    let state = quiet_state(overlay, neutral_mood());
    state.on_world_settled(WorldMsg::Seated(false));
    assert!(state.seated_on_ledge.borrow().is_none());
    assert!(state.ledge_sit_task.borrow().is_none());
}

// -- GET /world reports seated_on_ledge -----------------------------------------

#[test]
fn test_world_endpoint_reports_seated_on_ledge() {
    let world = WorldState {
        platforms: vec![ledge()],
        monitors: Vec::new(),
        updated_at: 0.0,
        current_platform_id: Some("win:1:0".to_string()),
        seated_on_ledge: Some("win:1:0".to_string()),
    };

    let report = world_endpoint(&world);

    assert_eq!(report.seated_on_ledge.as_deref(), Some("win:1:0"));
}

// -- overlay.py's {"seated": ...} plumbing (a real OverlayClient) --------------

#[test]
fn test_wait_for_seated_resolves_from_a_real_notice() {
    let mut client = OverlayClient::new();
    let waiter = client.add_seated_waiter();
    client.read_loop("{\"seated\": true}\n".as_bytes());
    assert_eq!(waiter.result(), Some(true));
}
