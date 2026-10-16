//! WP-16 music_start / music_high_energy / music_stop (audit 2026-09-24).

use std::cell::Cell;
use std::rc::Rc;

use felt_music_events::{HIGH_ENERGY_COOLDOWN_S, MusicEventTracker, STOP_GRACE_S};

/// Boxed injectable clock so the tracker type stays monomorphic in tests.
type BoxedClock = Box<dyn FnMut() -> f64>;

/// Manual test clock (Python's `_Clock`): its `t` is stepped by `run`.
#[derive(Default)]
struct Clock {
    t: Rc<Cell<f64>>,
}

impl Clock {
    fn new() -> Self {
        Self::default()
    }

    /// A clock function sharing this clock's `t`, for `MusicEventTracker::new`.
    fn now_fn(&self) -> BoxedClock {
        let t = Rc::clone(&self.t);
        Box::new(move || t.get())
    }

    fn tick(&self) {
        self.t.set(self.t.get() + 1.0);
    }
}

/// Steps the clock 1 s per tick for `seconds` ticks and collects the events.
fn run(
    tracker: &mut MusicEventTracker<BoxedClock>,
    clock: &Clock,
    seconds: f64,
    locked: bool,
    energy: f64,
) -> Vec<&'static str> {
    let mut events = Vec::new();
    for _ in 0..seconds as i64 {
        clock.tick();
        events.extend(tracker.update(locked, energy));
    }
    events
}

#[test]
fn test_start_once_and_brief_dropouts_do_not_stop() {
    let clock = Clock::new();
    let mut tracker = MusicEventTracker::new(clock.now_fn());
    let events = run(&mut tracker, &clock, 10.0, true, 0.1);
    assert_eq!(events, ["music_start"]);
    let events = run(&mut tracker, &clock, STOP_GRACE_S - 2.0, false, 0.1);
    assert_eq!(events, Vec::<&str>::new());
    // same session
    let events = run(&mut tracker, &clock, 5.0, true, 0.1);
    assert_eq!(events, Vec::<&str>::new());
}

#[test]
fn test_stop_after_the_grace_period() {
    let clock = Clock::new();
    let mut tracker = MusicEventTracker::new(clock.now_fn());
    run(&mut tracker, &clock, 10.0, true, 0.1);
    let events = run(&mut tracker, &clock, STOP_GRACE_S + 1.0, false, 0.0);
    assert_eq!(events, ["music_stop"]);
    assert!(!tracker.playing);
}

#[test]
fn test_high_energy_is_relative_sustained_and_rate_limited() {
    let clock = Clock::new();
    let mut tracker = MusicEventTracker::new(clock.now_fn());
    // settle the running average
    run(&mut tracker, &clock, 30.0, true, 0.1);
    // not sustained yet
    let events = run(&mut tracker, &clock, 3.0, true, 0.2);
    assert_eq!(events, Vec::<&str>::new());
    let events = run(&mut tracker, &clock, 6.0, true, 0.2);
    assert_eq!(events, ["music_high_energy"]);
    // cooldown
    let events = run(&mut tracker, &clock, 20.0, true, 0.25);
    assert_eq!(events, Vec::<&str>::new());
    run(&mut tracker, &clock, HIGH_ENERGY_COOLDOWN_S, true, 0.1);
    let events = run(&mut tracker, &clock, 10.0, true, 0.3);
    assert!(events.contains(&"music_high_energy"));
}

#[test]
fn test_steady_loud_music_is_not_high_energy() {
    let clock = Clock::new();
    let mut tracker = MusicEventTracker::new(clock.now_fn());
    let events = run(&mut tracker, &clock, 120.0, true, 0.5);
    assert_eq!(events, ["music_start"]);
}
