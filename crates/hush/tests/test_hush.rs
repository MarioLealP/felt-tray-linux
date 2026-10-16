//! HushState (WP-09): extend/cap/expire, driven by an injected fake clock
//! rather than a real 10-minute sleep.

use std::cell::Cell;
use std::rc::Rc;

use felt_hush::HushState;

fn clock(now: Rc<Cell<f64>>) -> impl Fn() -> f64 {
    move || now.get()
}

#[test]
fn test_inactive_by_default() {
    let hush = HushState::new(clock(Rc::new(Cell::new(0.0))));
    assert!(!hush.active());
    assert_eq!(hush.remaining_seconds(), 0.0);
}

#[test]
fn test_set_activates_hush_for_the_given_minutes() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    assert!(hush.active());
    assert!((hush.remaining_seconds() - 600.0).abs() < 1e-6);
}

#[test]
fn test_expires_after_its_duration() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    now.set(601.0);
    assert!(!hush.active());
}

#[test]
fn test_still_active_just_before_expiry() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    now.set(599.0);
    assert!(hush.active());
}

#[test]
fn test_a_second_call_extends_rather_than_resets() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    // 5 minutes into the first hush (5 min remaining)
    now.set(300.0);
    // extend by another 10 from the still-active deadline
    hush.set(10.0);
    // 5 min already remaining + 10 more = 15 min left from now
    assert!((hush.remaining_seconds() - 15.0 * 60.0).abs() < 1e-6);
}

#[test]
fn test_extension_is_capped_at_sixty_minutes_from_now() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    // 10 x 10min = 100min worth of extension requests
    for _ in 0..10 {
        hush.set(10.0);
    }
    assert!(hush.remaining_seconds() <= 60.0 * 60.0 + 1e-6);
}

#[test]
fn test_clear_deactivates_immediately() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    hush.clear();
    assert!(!hush.active());
}

#[test]
fn test_set_after_expiry_starts_fresh_not_from_the_old_deadline() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    // well past expiry
    now.set(700.0);
    hush.set(5.0);
    assert!((hush.remaining_seconds() - 5.0 * 60.0).abs() < 1e-6);
}

// -- clear()'s summary: what the unhush announce is phrased from ---------------

#[test]
fn test_clear_without_an_active_hush_returns_none() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    assert!(hush.clear().is_none());
    hush.set(1.0);
    // expired on its own
    now.set(61.0);
    assert!(hush.clear().is_none());
}

#[test]
fn test_clear_reports_an_early_unhush() {
    let now = Rc::new(Cell::new(100.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    now.set(100.0 + 95.0);
    let Some(ended) = hush.clear() else {
        panic!("expected a HushEnded summary");
    };
    assert!(ended.hushed_for_s == 95.0 && ended.planned_s == 600.0 && ended.early);
    assert!(!hush.active());
}

#[test]
fn test_clear_near_the_end_counts_as_serving_the_full_hush() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    now.set(590.0);
    let Some(ended) = hush.clear() else {
        panic!("expected a HushEnded summary");
    };
    assert!(!ended.early);
}

#[test]
fn test_planned_includes_extensions() {
    let now = Rc::new(Cell::new(0.0));
    let mut hush = HushState::new(clock(Rc::clone(&now)));
    hush.set(10.0);
    now.set(300.0);
    hush.set(10.0);
    now.set(400.0);
    let Some(ended) = hush.clear() else {
        panic!("expected a HushEnded summary");
    };
    assert!(ended.planned_s == 20.0 * 60.0 && ended.hushed_for_s == 400.0);
}

#[test]
fn test_consume_expiry_reports_a_lapsed_hush_once_as_served_in_full() {
    let clock_v = Rc::new(Cell::new(1000.0));
    let mut h = HushState::new(clock(Rc::clone(&clock_v)));
    h.set(10.0);
    // still running
    assert!(h.consume_expiry().is_none());
    clock_v.set(clock_v.get() + 10.0 * 60.0 + 1.0);
    let Some(ended) = h.consume_expiry() else {
        panic!("expected a HushEnded summary");
    };
    assert!(!ended.early);
    assert!(ended.hushed_for_s.round() == 600.0 && ended.planned_s.round() == 600.0);
    // exactly once
    assert!(h.consume_expiry().is_none());
    assert!(!h.active());
}

#[test]
fn test_consume_expiry_is_none_after_a_manual_unhush() {
    let clock_v = Rc::new(Cell::new(0.0));
    let mut h = HushState::new(clock(Rc::clone(&clock_v)));
    h.set(10.0);
    h.clear();
    clock_v.set(clock_v.get() + 10.0 * 60.0 + 1.0);
    assert!(h.consume_expiry().is_none());
}
