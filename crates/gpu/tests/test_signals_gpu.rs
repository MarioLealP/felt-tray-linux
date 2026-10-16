//! `signals.gpu.GpuHeatTracker` (WP-17): pure sustained-threshold logic
//! over synthetic temperature readings, no real WMI/hardware involved.

use felt_gpu::{GPU_HOT_COOLDOWN_S, GpuHeatTracker};
use std::cell::Cell;
use std::rc::Rc;

/// Mutable test clock - the Python `clock = [0.0]` + `lambda: clock[0]`
/// pattern: the returned handle advances time, the closure reads it.
fn clock(start: f64) -> (Rc<Cell<f64>>, impl Fn() -> f64) {
    let clock = Rc::new(Cell::new(start));
    let read = Rc::clone(&clock);
    (clock, move || read.get())
}

#[test]
fn test_no_fire_below_threshold() {
    let (_clock, now_fn) = clock(0.0);
    let mut tracker = GpuHeatTracker::new(80.0, 10.0, GPU_HOT_COOLDOWN_S, now_fn);
    for _ in 0..5 {
        assert!(!tracker.update(Some(70.0)));
    }
}

#[test]
fn test_no_fire_until_sustained() {
    let (clock, now_fn) = clock(0.0);
    let mut tracker = GpuHeatTracker::new(80.0, 10.0, GPU_HOT_COOLDOWN_S, now_fn);

    assert!(!tracker.update(Some(85.0))); // t=0
    clock.set(9.0);
    assert!(!tracker.update(Some(85.0))); // only 9s sustained
    clock.set(10.0);
    assert!(tracker.update(Some(85.0))); // a full 10s sustained
}

#[test]
fn test_a_dip_below_threshold_resets_the_sustain_timer() {
    let (clock, now_fn) = clock(0.0);
    let mut tracker = GpuHeatTracker::new(80.0, 10.0, GPU_HOT_COOLDOWN_S, now_fn);
    tracker.update(Some(85.0));
    clock.set(5.0);
    assert!(!tracker.update(Some(70.0))); // cooled off - resets
    clock.set(10.0);
    assert!(!tracker.update(Some(85.0))); // only just re-crossed at t=10
    clock.set(20.0);
    assert!(tracker.update(Some(85.0))); // a full 10s since the re-cross
}

#[test]
fn test_none_reading_is_treated_as_not_hot() {
    let mut tracker = GpuHeatTracker::new(80.0, 10.0, GPU_HOT_COOLDOWN_S, || 0.0);
    assert!(!tracker.update(None));
}

#[test]
fn test_does_not_refire_within_cooldown() {
    let (clock, now_fn) = clock(0.0);
    let mut tracker = GpuHeatTracker::new(80.0, 10.0, 300.0, now_fn);
    tracker.update(Some(85.0));
    clock.set(10.0);
    assert!(tracker.update(Some(85.0)));
    clock.set(60.0);
    assert!(!tracker.update(Some(85.0))); // still hot, but within the 300s cooldown
}

#[test]
fn test_fires_again_after_cooldown_elapses() {
    let (clock, now_fn) = clock(0.0);
    let mut tracker = GpuHeatTracker::new(80.0, 10.0, 300.0, now_fn);
    tracker.update(Some(85.0));
    clock.set(10.0);
    assert!(tracker.update(Some(85.0)));
    clock.set(320.0);
    assert!(tracker.update(Some(85.0))); // 300s cooldown elapsed, still hot
}
