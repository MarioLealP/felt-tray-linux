//! Port of ref/felt-animation/tests/test_nav_executor.py: nav_executor's
//! run_plan against fake overlays (the structural stand-in for
//! BehaviorState's OverlayLike Protocol, plus the hop/climb/
//! wait_for_landed/wait_for_climbed methods nav_executor calls directly on
//! state.overlay).

use std::cell::RefCell;
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use felt_nav_executor::{
    AppState, Climb, ClimbedNotice, Geometry, HopDown, LandedNotice, NoopStandUpHooks, Overlay,
    Platform, Step, Walk, run_plan,
};

// -- a std-only block_on to drive the async API in tests ----------------------

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => thread::park(),
        }
    }
}

// -- the Python spec's FakeOverlay / _Geometry / _make_state ------------------

struct FakeOverlay {
    moves: RefCell<Vec<f64>>,
    hops: RefCell<Vec<f64>>,
    climbs: RefCell<Vec<(f64, i32)>>,
    landed: Option<LandedNotice>,
    climbed: Option<ClimbedNotice>,
}

impl FakeOverlay {
    /// FakeOverlay(landed=None) in the Python - the other notice keeps its
    /// default.
    fn with_landed(landed: Option<LandedNotice>) -> Self {
        Self {
            moves: RefCell::new(Vec::new()),
            hops: RefCell::new(Vec::new()),
            climbs: RefCell::new(Vec::new()),
            landed,
            climbed: Some(ClimbedNotice { y_px: 1.0 }),
        }
    }
}

impl Default for FakeOverlay {
    fn default() -> Self {
        Self::with_landed(Some(LandedNotice { height_px: 10.0 }))
    }
}

impl Overlay for FakeOverlay {
    async fn play_and_wait(&self, _tag: &str, _timeout: f64, _clip: Option<&str>) {}

    async fn play_tag(&self, _tag: &str, _looping: bool, _clip: Option<&str>) {}

    async fn move_to(&self, x: f64, _offscreen: bool, _running: bool) {
        self.moves.borrow_mut().push(x);
    }

    async fn wait_for_arrived(&self, _timeout: f64) {}

    async fn send_tag(&self, _tag: &str) -> bool {
        true
    }

    async fn hop(&self, y_px: f64) -> bool {
        self.hops.borrow_mut().push(y_px);
        true
    }

    async fn climb(&self, y_px: f64, meters: i32) -> bool {
        self.climbs.borrow_mut().push((y_px, meters));
        true
    }

    async fn wait_for_landed(&self, _timeout: f64) -> Option<LandedNotice> {
        self.landed
    }

    async fn wait_for_climbed(&self, _timeout: f64) -> Option<ClimbedNotice> {
        self.climbed
    }
}

struct FakeGeometry;

impl Geometry for FakeGeometry {
    fn px_to_world_x(&self, x_px: f64) -> f64 {
        x_px / 100.0
    }
}

fn make_state(
    overlay: Rc<FakeOverlay>,
    platforms: Vec<Platform>,
) -> AppState<FakeOverlay, NoopStandUpHooks, FakeGeometry> {
    // A duck-typed stand-in, same spirit as the Python's SimpleNamespace
    // config fake: AppState::new fills in exactly the defaults _make_state
    // sets (known_tags empty, current_x 0.0, pending_platform_id None,
    // seated_on_ledge None).
    AppState::new(overlay, NoopStandUpHooks, FakeGeometry, platforms)
}

fn floor() -> Platform {
    Platform {
        id: "floor:0".to_string(),
        hwnd: None,
        x0: 0.0,
        x1: 1920.0,
        y: 1040.0,
        kind: "floor".to_string(),
    }
}

fn ledge() -> Platform {
    Platform {
        id: "win:1:0".to_string(),
        hwnd: Some(1),
        x0: 100.0,
        x1: 900.0,
        y: 600.0,
        kind: "window".to_string(),
    }
}

// -- the tests ---------------------------------------------------------------

#[test]
fn test_walk_only_plan_moves_and_succeeds() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::default());
        let state = make_state(Rc::clone(&overlay), vec![floor()]);
        let ok = run_plan(&state, &[Step::Walk(Walk { x_px: 500.0 })]).await;
        assert!(ok);
        assert_eq!(state.current_x.get(), 5.0); // 500 / 100 (the fake px->world map)
        assert!(!state.behavior.base_busy());
    })
}

#[test]
fn test_hop_down_step_sends_hop_and_waits_for_landed() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::default());
        let state = make_state(Rc::clone(&overlay), vec![ledge(), floor()]);
        let ok = run_plan(
            &state,
            &[
                Step::Walk(Walk { x_px: 500.0 }),
                Step::HopDown(HopDown {
                    to_platform: "floor:0".to_string(),
                    x_px: 500.0,
                }),
                Step::Walk(Walk { x_px: 1000.0 }),
            ],
        )
        .await;
        assert!(ok);
        assert_eq!(*overlay.hops.borrow(), [floor().y]);
        assert_eq!(
            state.pending_platform_id.borrow().as_deref(),
            Some("floor:0")
        );
    })
}

#[test]
fn test_climb_step_sends_climb_with_meters_and_waits_for_climbed() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::default());
        let state = make_state(Rc::clone(&overlay), vec![floor(), ledge()]);
        let ok = run_plan(
            &state,
            &[
                Step::Walk(Walk { x_px: 500.0 }),
                Step::Climb(Climb {
                    to_platform: "win:1:0".to_string(),
                    x_px: 500.0,
                    meters: 2,
                }),
                Step::Walk(Walk { x_px: 600.0 }),
            ],
        )
        .await;
        assert!(ok);
        assert_eq!(*overlay.climbs.borrow(), [(ledge().y, 2)]);
        assert_eq!(
            state.pending_platform_id.borrow().as_deref(),
            Some("win:1:0")
        );
    })
}

#[test]
fn test_hop_step_timeout_aborts_the_plan() {
    block_on(async {
        // landed=None simulates wait_for_landed() timing out.
        let overlay = Rc::new(FakeOverlay::with_landed(None));
        let state = make_state(Rc::clone(&overlay), vec![ledge(), floor()]);
        let ok = run_plan(
            &state,
            &[
                Step::Walk(Walk { x_px: 500.0 }),
                Step::HopDown(HopDown {
                    to_platform: "floor:0".to_string(),
                    x_px: 500.0,
                }),
                Step::Walk(Walk { x_px: 1000.0 }),
            ],
        )
        .await;
        assert!(!ok);
        // the takeoff Walk ran; the post-hop Walk never did
        assert_eq!(*overlay.moves.borrow(), [5.0]);
    })
}

#[test]
fn test_missing_target_platform_aborts_the_plan() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::default());
        // "floor:0" isn't in the snapshot anymore
        let state = make_state(Rc::clone(&overlay), vec![ledge()]);
        let ok = run_plan(
            &state,
            &[Step::HopDown(HopDown {
                to_platform: "floor:0".to_string(),
                x_px: 500.0,
            })],
        )
        .await;
        assert!(!ok);
        assert!(overlay.hops.borrow().is_empty());
    })
}

#[test]
fn test_base_lock_is_held_for_the_whole_plan() {
    block_on(async {
        let overlay = Rc::new(FakeOverlay::default());
        let state = make_state(Rc::clone(&overlay), vec![floor()]);
        assert!(!state.behavior.base_busy());
        let ok = run_plan(
            &state,
            &[
                Step::Walk(Walk { x_px: 100.0 }),
                Step::Walk(Walk { x_px: 200.0 }),
            ],
        )
        .await;
        assert!(ok);
        assert!(!state.behavior.base_busy()); // released once the plan finishes
    })
}
