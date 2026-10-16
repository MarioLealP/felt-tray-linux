//! Port of the BehaviorState-facing tests of
//! ref/felt-animation/tests/test_layers.py: the base/upper lock split
//! (WP-20), plus test_unreported_tags_count_as_full_body (layer_for, which
//! lives in this crate as a shared type). Test names match the Python ones.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use felt_state::exec::{self, AsyncEvent};
use felt_state::{BehaviorState, MonotonicClock, OverlayLike, layer_for};

fn layers() -> HashMap<String, String> {
    HashMap::from([
        ("idle".to_string(), "base".to_string()),
        ("idle/sitting".to_string(), "base".to_string()),
        (
            "locomotion/walk/normal_walk".to_string(),
            "base".to_string(),
        ),
        ("gesture/wave".to_string(), "upper".to_string()),
        ("gesture/thinking".to_string(), "upper".to_string()),
        ("emotion/happy".to_string(), "upper".to_string()),
        ("reaction/turn_surprised".to_string(), "full".to_string()),
        ("transition/stand_to_sit".to_string(), "full".to_string()),
    ])
}

async fn settle() {
    for _ in 0..5 {
        exec::yield_now().await;
    }
}

#[test]
fn test_unreported_tags_count_as_full_body() {
    // An older overlay reports nothing: every tag takes both locks, i.e. the
    // pre-WP-20 single-lock behaviour.
    assert_eq!(layer_for("gesture/wave", &HashMap::new()), "full");
    assert_eq!(layer_for("gesture/wave", &layers()), "upper");
    assert_eq!(layer_for("not/a/tag", &layers()), "full");
}

/// Clips 'play' until release(tag) - so a test can hold one layer busy
/// and see what else gets to start meanwhile. Implements BehaviorState's
/// `OverlayLike` Protocol directly.
type TagLayerFn = Box<dyn Fn(&str) -> String>;

struct LayeredOverlay {
    started: RefCell<Vec<String>>,
    gates: RefCell<HashMap<String, AsyncEvent>>,
    arrived: AsyncEvent,
    // A swappable layer-lookup, not a `tag_layer` override - so
    // test_without_a_layer_report_everything_shares_one_lock can swap the
    // *logic* out.
    tag_layer_fn: RefCell<TagLayerFn>,
}

impl LayeredOverlay {
    fn new() -> Self {
        let default_fn: TagLayerFn = Box::new(|tag: &str| layer_for(tag, &layers()));
        Self {
            started: RefCell::new(Vec::new()),
            gates: RefCell::new(HashMap::new()),
            arrived: AsyncEvent::new(),
            tag_layer_fn: RefCell::new(default_fn),
        }
    }

    fn release(&self, tag: &str) {
        self.gate(tag).set();
    }

    fn gate(&self, tag: &str) -> AsyncEvent {
        self.gates
            .borrow_mut()
            .entry(tag.to_string())
            .or_default()
            .clone()
    }
}

impl OverlayLike for LayeredOverlay {
    async fn play_and_wait(&self, tag: &str, _timeout: f64, _clip: Option<&str>) {
        self.started.borrow_mut().push(tag.to_string());
        self.gate(tag).wait().await;
    }

    async fn play_tag(&self, tag: &str, _looped: bool) {
        self.started.borrow_mut().push(tag.to_string());
    }

    async fn move_to(&self, x: f64, _offscreen: bool, _running: bool) {
        self.started.borrow_mut().push(format!("move {x:?}"));
    }

    async fn wait_for_arrived(&self, _timeout: f64) {
        self.arrived.wait().await;
    }

    fn tag_layer(&self, tag: &str) -> Option<String> {
        let f = self.tag_layer_fn.borrow();
        Some(f.as_ref()(tag))
    }
}

// -- the locks -----------------------------------------------------------------

#[test]
fn test_wave_plays_while_walking() {
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let walk = exec::spawn(async move {
            s.walk_to(
                1.0,
                Some("locomotion/walk/normal_walk"),
                false,
                None,
                None,
                false,
                5.0,
            )
            .await
        });
        settle().await;
        assert!(state.base_busy() && !state.upper_busy());

        let s = Rc::clone(&state);
        let wave = exec::spawn(async move { s.run_sequence(&["gesture/wave"], 5.0, None).await });
        settle().await;
        // started mid-walk, not queued
        assert!(
            overlay
                .started
                .borrow()
                .contains(&"gesture/wave".to_string())
        );
        assert!(!walk.done());

        overlay.release("gesture/wave");
        let _ = wave.join().await;
        overlay.arrived.set();
        let _ = walk.join().await;
        assert!(!state.busy());
    });
}

#[test]
fn test_full_body_tag_waits_for_the_walk() {
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let walk = exec::spawn(async move {
            s.walk_to(
                1.0,
                Some("locomotion/walk/normal_walk"),
                false,
                None,
                None,
                false,
                5.0,
            )
            .await
        });
        settle().await;

        let s = Rc::clone(&state);
        let turn = exec::spawn(async move {
            s.run_sequence(&["reaction/turn_surprised"], 5.0, None)
                .await
        });
        settle().await;
        assert!(
            !overlay
                .started
                .borrow()
                .contains(&"reaction/turn_surprised".to_string())
        );

        overlay.arrived.set();
        let _ = walk.join().await;
        settle().await;
        assert_eq!(
            overlay.started.borrow().last().map(String::as_str),
            Some("reaction/turn_surprised")
        );
        overlay.release("reaction/turn_surprised");
        let _ = turn.join().await;
    });
}

#[test]
fn test_full_body_tag_waits_for_a_gesture_and_blocks_the_next_walk() {
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let wave = exec::spawn(async move { s.run_sequence(&["gesture/wave"], 5.0, None).await });
        settle().await;

        // Takes the base lock, then waits for the upper one: "never interrupt".
        let s = Rc::clone(&state);
        let turn = exec::spawn(async move {
            s.run_sequence(&["reaction/turn_surprised"], 5.0, None)
                .await
        });
        settle().await;
        assert_eq!(
            *overlay.started.borrow(),
            ["gesture/wave"].map(String::from)
        );
        assert!(state.base_busy());

        overlay.release("gesture/wave");
        let _ = wave.join().await;
        settle().await;
        assert_eq!(
            *overlay.started.borrow(),
            ["gesture/wave", "reaction/turn_surprised"].map(String::from)
        );
        overlay.release("reaction/turn_surprised");
        let _ = turn.join().await;
        assert!(!state.busy());
    });
}

#[test]
fn test_upper_tags_still_queue_behind_each_other() {
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let wave = exec::spawn(async move { s.run_sequence(&["gesture/wave"], 5.0, None).await });
        let s = Rc::clone(&state);
        let happy = exec::spawn(async move { s.run_sequence(&["emotion/happy"], 5.0, None).await });
        settle().await;
        assert_eq!(
            *overlay.started.borrow(),
            ["gesture/wave"].map(String::from)
        );
        overlay.release("gesture/wave");
        let _ = wave.join().await;
        settle().await;
        assert_eq!(
            *overlay.started.borrow(),
            ["gesture/wave", "emotion/happy"].map(String::from)
        );
        overlay.release("emotion/happy");
        let _ = happy.join().await;
    });
}

#[test]
fn test_mixed_sequence_takes_both_locks() {
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let seq = exec::spawn(async move {
            s.run_sequence(&["transition/stand_to_sit", "gesture/wave"], 5.0, None)
                .await
        });
        settle().await;
        assert!(state.base_busy() && state.upper_busy());
        overlay.release("transition/stand_to_sit");
        overlay.release("gesture/wave");
        let _ = seq.join().await;
    });
}

#[test]
fn test_thinking_loop_runs_during_a_walk() {
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let walk = exec::spawn(async move {
            s.walk_to(
                1.0,
                Some("locomotion/walk/normal_walk"),
                false,
                None,
                None,
                false,
                5.0,
            )
            .await
        });
        settle().await;

        state.begin_thinking("gesture/thinking", 5.0);
        settle().await;
        assert!(
            overlay
                .started
                .borrow()
                .contains(&"gesture/thinking".to_string())
        );
        assert!(state.upper_busy());

        let s = Rc::clone(&state);
        let stop = exec::spawn(async move { s.end_thinking_and_wait().await });
        settle().await;
        overlay.release("gesture/thinking");
        exec::wait_for(stop.join(), std::time::Duration::from_secs_f64(1.0))
            .await
            .expect("end_thinking_and_wait timed out");
        assert!(!state.upper_busy());
        overlay.arrived.set();
        let _ = walk.join().await;
    });
}

#[test]
fn test_without_a_layer_report_everything_shares_one_lock() {
    // FakeOverlay-style client with no tag_layer at all (or an old overlay).
    exec::block_on(async {
        let overlay = Rc::new(LayeredOverlay::new());
        *overlay.tag_layer_fn.borrow_mut() = Box::new(|tag: &str| layer_for(tag, &HashMap::new()));
        let state = Rc::new(BehaviorState::new(
            Rc::clone(&overlay),
            Rc::new(MonotonicClock::new()),
        ));
        let s = Rc::clone(&state);
        let walk = exec::spawn(async move {
            s.walk_to(
                1.0,
                Some("locomotion/walk/normal_walk"),
                false,
                None,
                None,
                false,
                5.0,
            )
            .await
        });
        settle().await;
        let s = Rc::clone(&state);
        let wave = exec::spawn(async move { s.run_sequence(&["gesture/wave"], 5.0, None).await });
        settle().await;
        // waits for the walk, as before WP-20
        assert!(
            !overlay
                .started
                .borrow()
                .contains(&"gesture/wave".to_string())
        );
        overlay.arrived.set();
        let _ = walk.join().await;
        settle().await;
        overlay.release("gesture/wave");
        let _ = wave.join().await;
    });
}
