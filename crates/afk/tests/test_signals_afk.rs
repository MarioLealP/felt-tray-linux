//! signals.afk.AfkTracker (WP-17): pure threshold-crossing logic over
//! synthetic idle-time readings, no real OS input state involved.

use std::cell::Cell;

use felt_afk::AfkTracker;

#[test]
fn test_no_event_below_threshold() {
    let clock = Cell::new(0.0);
    let mut tracker = AfkTracker::new(600.0, || clock.get());
    assert!(tracker.update(60.0).is_none());
    assert!(!tracker.afk);
}

#[test]
fn test_afk_start_fires_once_when_threshold_is_crossed() {
    let clock = Cell::new(1000.0);
    let mut tracker = AfkTracker::new(600.0, || clock.get());

    let event = tracker.update(650.0); // already idle 650s when first noticed

    assert_eq!(event, Some(("afk_start", 0.0)));
    assert!(tracker.afk);
}

#[test]
fn test_afk_start_does_not_refire_while_still_idle() {
    let clock = Cell::new(1000.0);
    let mut tracker = AfkTracker::new(600.0, || clock.get());
    let _ = tracker.update(650.0);

    clock.set(1100.0);
    assert!(tracker.update(750.0).is_none());
}

#[test]
fn test_afk_end_fires_when_input_resumes_and_reports_duration() {
    let clock = Cell::new(1000.0);
    let mut tracker = AfkTracker::new(600.0, || clock.get());
    let _ = tracker.update(650.0); // afk started effectively at t=1000-650=350

    clock.set(1500.0); // 500s later, Mario moves the mouse
    let event = tracker.update(0.5);

    assert!(event.is_some());
    let event = event.unwrap();
    assert_eq!(event.0, "afk_end");
    assert_eq!(event.1, 1150.0); // 1500 - 350
    assert!(!tracker.afk);
}

#[test]
fn test_full_cycle_can_repeat() {
    let clock = Cell::new(0.0);
    let mut tracker = AfkTracker::new(600.0, || clock.get());

    clock.set(600.0);
    let event = tracker.update(600.0);
    assert!(event.is_some() && event.unwrap().0 == "afk_start");
    clock.set(700.0);
    let event = tracker.update(0.0);
    assert!(event.is_some() && event.unwrap().0 == "afk_end");

    clock.set(1400.0);
    let event = tracker.update(600.0);
    assert!(event.is_some() && event.unwrap().0 == "afk_start");
    clock.set(1500.0);
    let event = tracker.update(0.0);
    assert!(event.is_some() && event.unwrap().0 == "afk_end");
}
