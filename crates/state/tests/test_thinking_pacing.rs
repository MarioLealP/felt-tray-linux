//! Port of test_thinking_loop_with_overlay_down_neither_blocks_nor_spins
//! from ref/felt-animation/tests/test_overlay_connection.py. The Python test
//! drives a real OverlayClient with the relay down (play_and_wait returns
//! immediately, no clip ever finishes); this port stands that down overlay in
//! with a fake whose play_and_wait returns at once - same BehaviorState
//! assertions. Test name matches the Python one.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use felt_state::exec;
use felt_state::{BehaviorState, MonotonicClock, OverlayLike};

/// A disconnected-overlay double: play_and_wait comes straight back, like
/// OverlayClient with the relay down.
struct DownOverlay {
    calls: Cell<usize>,
}

impl OverlayLike for DownOverlay {
    async fn play_and_wait(&self, _tag: &str, _timeout: f64, _clip: Option<&str>) {
        self.calls.set(self.calls.get() + 1);
    }

    async fn play_tag(&self, _tag: &str, _looped: bool) {
        unimplemented!("not exercised by this test")
    }

    async fn move_to(&self, _x: f64, _offscreen: bool, _running: bool) {
        unimplemented!("not exercised by this test")
    }

    async fn wait_for_arrived(&self, _timeout: f64) {
        unimplemented!("not exercised by this test")
    }
}

#[test]
fn test_thinking_loop_with_overlay_down_neither_blocks_nor_spins() {
    exec::block_on(async {
        let overlay = Rc::new(DownOverlay {
            calls: Cell::new(0),
        });
        let state = BehaviorState::new(Rc::clone(&overlay), Rc::new(MonotonicClock::new()));
        state.begin_thinking("gesture/thinking", 30.0);
        exec::sleep(Duration::from_millis(300)).await;
        let start = Instant::now();
        state.end_thinking_and_wait().await;

        assert!(start.elapsed() < Duration::from_millis(200)); // stops promptly, no 30 s timeout
        assert!(overlay.calls.get() <= 2); // paced, not a busy loop
    });
}
