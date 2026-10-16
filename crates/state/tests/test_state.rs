//! Port of ref/felt-animation/tests/test_state.py - the spec for
//! `felt_state::BehaviorState`. Test names match the Python ones.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use felt_state::exec::{self, AsyncEvent};
use felt_state::{BehaviorState, MonotonicClock, OverlayLike};

/// Implements only what BehaviorState's `OverlayLike` Protocol needs.
/// No `tag_layer` at all (unlike LayeredOverlay in test_layers_locks.rs) -
/// layer_of()'s own getattr-based lookup falls back to layer_for(tag, {}),
/// the pre-WP-20 single-lock behaviour these tests rely on.
struct FakeOverlay {
    calls: RefCell<Vec<String>>,
}

impl FakeOverlay {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl OverlayLike for FakeOverlay {
    async fn play_and_wait(&self, tag: &str, _timeout: f64, _clip: Option<&str>) {
        self.calls.borrow_mut().push(tag.to_string());
        exec::sleep(Duration::from_millis(10)).await;
    }

    async fn play_tag(&self, _tag: &str, _looped: bool) {
        unimplemented!("not exercised by this fake's tests")
    }

    async fn move_to(&self, _x: f64, _offscreen: bool, _running: bool) {
        unimplemented!("not exercised by this fake's tests")
    }

    async fn wait_for_arrived(&self, _timeout: f64) {
        unimplemented!("not exercised by this fake's tests")
    }
}

#[test]
fn test_run_sequence_holds_the_lock() {
    exec::block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));

        let s = Rc::clone(&state);
        let task = exec::spawn(async move { s.run_sequence(&["a", "b"], 1.0, None).await });
        exec::yield_now().await; // let run_sequence acquire the lock

        assert!(state.busy());
        let _ = task.join().await;
        assert!(!state.busy());
        assert_eq!(*overlay.calls.borrow(), ["a", "b"].map(String::from));
    });
}

#[test]
fn test_begin_thinking_idle_is_a_no_op() {
    exec::block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));

        state.begin_thinking("idle", 1.0);
        state.end_thinking_and_wait().await; // must return immediately, no task was started

        assert!(overlay.calls.borrow().is_empty());
        assert!(!state.busy());
    });
}

/// Regression: end_thinking_and_wait() called before the _loop() task
/// has actually run must not race the stop-signal clear() inside _loop()
/// into clearing a signal that was already set - which would spin forever
/// since nothing would ever set it again.
#[test]
fn test_begin_thinking_then_immediate_end_does_not_loop_forever() {
    exec::block_on(async {
        let overlay = Rc::new(FakeOverlay::new());
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));

        state.begin_thinking("gesture/thinking", 1.0);
        exec::wait_for(state.end_thinking_and_wait(), Duration::from_secs_f64(2.0))
            .await
            .expect("end_thinking_and_wait timed out");

        assert!(!state.busy());
    });
}

// -- walk_to (WP-18) --------------------------------------------------------

#[derive(Debug, PartialEq)]
struct MoveRec {
    x: f64,
    offscreen: bool,
    running: bool,
}

struct FakeWalkOverlay {
    calls: RefCell<Vec<String>>,
    moves: RefCell<Vec<MoveRec>>,
    arrived: AsyncEvent,
    loops: RefCell<Vec<String>>,
    arrive_on_move: Cell<bool>,
    armed: Cell<bool>,
}

impl FakeWalkOverlay {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            moves: RefCell::new(Vec::new()),
            arrived: AsyncEvent::new(),
            loops: RefCell::new(Vec::new()),
            arrive_on_move: Cell::new(false),
            armed: Cell::new(false),
        }
    }

    fn fire_arrived(&self) {
        self.arrived.set();
    }
}

impl OverlayLike for FakeWalkOverlay {
    async fn play_and_wait(&self, tag: &str, _timeout: f64, _clip: Option<&str>) {
        self.calls.borrow_mut().push(tag.to_string());
        exec::sleep(Duration::from_millis(10)).await;
    }

    async fn play_tag(&self, tag: &str, looped: bool) {
        self.calls.borrow_mut().push(tag.to_string());
        if looped {
            self.loops.borrow_mut().push(tag.to_string());
        }
    }

    async fn move_to(&self, x: f64, offscreen: bool, running: bool) {
        self.moves.borrow_mut().push(MoveRec {
            x,
            offscreen,
            running,
        });
        // main.gd answers a zero-distance move at once - only a waiter that
        // is already registered sees it (like OverlayClient).
        if self.arrive_on_move.get() && self.armed.get() {
            self.arrived.set();
        }
    }

    async fn wait_for_arrived(&self, _timeout: f64) {
        self.armed.set(true);
        self.arrived.wait().await;
    }
}

#[test]
fn test_walk_to_plays_start_and_stop_around_the_move() {
    exec::block_on(async {
        let overlay = Rc::new(FakeWalkOverlay::new());
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));
        overlay.fire_arrived(); // no cycle tag - resolves as soon as arrived is set

        exec::wait_for(
            state.walk_to(1.0, None, false, Some("start"), Some("stop"), false, 1.0),
            Duration::from_secs_f64(1.0),
        )
        .await
        .expect("walk_to timed out");

        assert_eq!(*overlay.calls.borrow(), ["start", "stop"].map(String::from));
        assert_eq!(
            *overlay.moves.borrow(),
            vec![MoveRec {
                x: 1.0,
                offscreen: false,
                running: false
            }]
        );
    });
}

#[test]
fn test_walk_to_loops_the_cycle_clip_until_arrived() {
    exec::block_on(async {
        let overlay = Rc::new(FakeWalkOverlay::new());
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));

        let fire_overlay = Rc::clone(&overlay);
        exec::spawn(async move {
            exec::sleep(Duration::from_millis(30)).await;
            fire_overlay.fire_arrived();
        });
        exec::wait_for(
            state.walk_to(
                2.0,
                Some("walk"),
                false,
                Some("start"),
                Some("stop"),
                false,
                1.0,
            ),
            Duration::from_secs_f64(2.0),
        )
        .await
        .expect("walk_to timed out");

        // The cycle goes out once, looping - never re-sent per cycle.
        assert_eq!(
            *overlay.calls.borrow(),
            ["start", "walk", "stop"].map(String::from)
        );
        assert_eq!(*overlay.loops.borrow(), ["walk"].map(String::from));
    });
}

#[test]
fn test_walk_to_catches_an_instant_arrived_reply() {
    exec::block_on(async {
        let overlay = Rc::new(FakeWalkOverlay::new());
        overlay.arrive_on_move.set(true);
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));

        exec::wait_for(
            state.walk_to(0.0, Some("walk"), false, None, None, false, 5.0),
            Duration::from_secs_f64(1.0),
        )
        .await
        .expect("walk_to timed out");

        assert_eq!(*overlay.calls.borrow(), ["walk"].map(String::from));
    });
}

#[test]
fn test_walk_to_passes_the_running_flag_to_move() {
    exec::block_on(async {
        let overlay = Rc::new(FakeWalkOverlay::new());
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));
        overlay.fire_arrived();

        state.walk_to(1.0, None, true, None, None, false, 1.0).await;

        assert!(overlay.moves.borrow()[0].running);
    });
}

#[test]
fn test_walk_to_holds_the_lock_for_the_whole_sequence() {
    exec::block_on(async {
        let overlay = Rc::new(FakeWalkOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));

        let s = Rc::clone(&state);
        let task = exec::spawn(async move {
            s.walk_to(1.0, None, false, Some("start"), Some("stop"), false, 1.0)
                .await
        });
        exec::yield_now().await;
        assert!(state.busy());

        overlay.fire_arrived();
        let _ = task.join().await;
        assert!(!state.busy());
    });
}
