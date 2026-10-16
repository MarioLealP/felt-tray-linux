//! WP-14 gaze decisions (audit 2026-09-24): felt-animation decides, main.gd
//! executes. Pure rules + the send-on-change controller with a fake clock.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use felt_gaze::{Clock, DEFAULT_OPT_OUT_TAGS, GazeController, GazeMode, decide_gaze, is_opted_out};

#[test]
fn test_priority_order() {
    assert_eq!(decide_gaze(true, true, true, 9.0).0, GazeMode::Off);
    assert_eq!(decide_gaze(false, true, true, 0.0).0, GazeMode::Camera);
    assert_eq!(
        decide_gaze(false, false, true, 9.0),
        (GazeMode::Point, Some(0.35), Some(0.3))
    );
    assert_eq!(decide_gaze(false, false, false, 4.0).0, GazeMode::Camera);
    assert_eq!(decide_gaze(false, false, false, 3.9).0, GazeMode::Cursor);
}

#[test]
fn test_opt_out_matches_tag_prefixes_only() {
    assert!(is_opted_out(Some("air/fall"), DEFAULT_OPT_OUT_TAGS));
    assert!(is_opted_out(Some("dance"), DEFAULT_OPT_OUT_TAGS));
    assert!(is_opted_out(Some("gesture/nodding"), DEFAULT_OPT_OUT_TAGS));
    assert!(!is_opted_out(Some("gesture/wave"), DEFAULT_OPT_OUT_TAGS));
    assert!(!is_opted_out(Some("airplane"), DEFAULT_OPT_OUT_TAGS)); // prefix match is per path segment
    assert!(!is_opted_out(None, DEFAULT_OPT_OUT_TAGS));
}

/// Shared fake clock, advanced from the test while the controller holds it.
#[derive(Clone)]
struct _Clock {
    t: Rc<Cell<f64>>,
}

impl _Clock {
    fn new() -> Self {
        Self {
            t: Rc::new(Cell::new(100.0)),
        }
    }
}

impl Clock for _Clock {
    fn now(&self) -> f64 {
        self.t.get()
    }
}

#[test]
fn test_controller_sends_only_on_change_and_follows_the_cursor() {
    let clock = _Clock::new();
    let sent: Rc<RefCell<Vec<GazeMode>>> = Rc::new(RefCell::new(Vec::new()));
    let sent_log = Rc::clone(&sent);

    let mut gaze = GazeController::new(
        move |mode, _x, _y| {
            sent_log.borrow_mut().push(mode);
            Some(true)
        },
        clock.clone(),
    );
    gaze.observe_cursor(Some((10, 10)));
    gaze.update();
    gaze.update();
    assert_eq!(*sent.borrow(), [GazeMode::Cursor]);

    clock.t.set(clock.t.get() + 4.5); // cursor still for 4.5 s
    gaze.observe_cursor(Some((10, 10)));
    gaze.update();
    assert_eq!(*sent.borrow(), [GazeMode::Cursor, GazeMode::Camera]);

    gaze.observe_cursor(Some((12, 10))); // moved again
    gaze.update();
    assert_eq!(
        *sent.borrow(),
        [GazeMode::Cursor, GazeMode::Camera, GazeMode::Cursor]
    );

    gaze.speaking = true;
    gaze.update();
    gaze.current_tag = Some("air/fall".to_string());
    gaze.update();
    gaze.current_tag = None;
    gaze.speaking = false;
    gaze.update();
    assert_eq!(
        *sent.borrow(),
        [
            GazeMode::Cursor,
            GazeMode::Camera,
            GazeMode::Cursor,
            GazeMode::Camera,
            GazeMode::Off,
            GazeMode::Cursor
        ]
    );
}

#[test]
fn test_failed_send_is_retried_and_resend_repeats_the_current_mode() {
    let sent: Rc<RefCell<Vec<GazeMode>>> = Rc::new(RefCell::new(Vec::new()));
    let connected = Rc::new(Cell::new(false));
    let sent_log = Rc::clone(&sent);
    let connected_flag = Rc::clone(&connected);

    let mut gaze = GazeController::new(
        move |mode, _x, _y| {
            sent_log.borrow_mut().push(mode);
            Some(connected_flag.get())
        },
        _Clock::new(),
    );
    gaze.update();
    connected.set(true);
    gaze.update(); // retried because the first send wasn't delivered
    gaze.update(); // now delivered - no repeat
    gaze.resend(); // overlay reconnected
    assert_eq!(
        *sent.borrow(),
        [GazeMode::Cursor, GazeMode::Cursor, GazeMode::Cursor]
    );
}
