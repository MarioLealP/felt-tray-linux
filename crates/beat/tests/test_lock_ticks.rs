//! Regression (2026-10-16, live on Linux): with one estimate a second (slightly over, like a real loop), a
//! steady tempo must acquire the lock - the window used to be exactly LOCK_HOLD_S, so its span never got there.

use felt_beat::TempoLock;

#[test]
fn steady_estimates_once_a_second_acquire_the_lock() {
    let mut lock = TempoLock::new();
    let mut locked_at = None;
    for i in 0..12 {
        let now = f64::from(i) * 1.003;
        if lock.update(Some((78.6, 0.47)), Some(now)) && locked_at.is_none() {
            locked_at = Some(i);
        }
    }
    let at = locked_at.expect("locks on a steady tempo");
    assert!(at <= 7, "took {at} ticks");
    assert!((lock.bpm - 78.6).abs() < 0.01);
}
