//! Minimal stand-ins for the two `behavior.py` functions the ledge sit
//! tests exercise. Only the ledge-relevant paths are ported; the WP-21
//! chair-sit clip sequences, the known_tags cycle-tag selection and the
//! `{"arrived"}` waiter machinery stay in behavior.py / state.py.

use std::rc::Rc;

use crate::runtime::AppState;
use crate::shared::Posture;

/// `behavior.walk_to` down to what the ledge tests check: she always
/// stands up first ("she just moves while sitting" was a bug), then the
/// move goes out under the base lock with `current_x` bookkeeping.
pub async fn walk_to(state: &Rc<AppState>, target_x: f64, cycle_tag: &str, offscreen: bool) {
    // Mario, 2026-09-25: stand up first, always - presence walks used to
    // slide her across the screen still seated.
    stand_up_if_sitting(state).await;
    state.current_x.set(target_x);
    let running = cycle_tag == "locomotion/run";
    let _guard = state.behavior.hold_base().await;
    state.overlay.move_to(target_x, offscreen, running).await;
}

/// `behavior.stand_up_if_sitting` - the "walk_to stands her up first"
/// path. A ledge sit is a different posture mechanism than the WP-21
/// floor chair (main.gd's hips-pinned y-positioning), so it's checked
/// and cleared separately first.
pub async fn stand_up_if_sitting(state: &Rc<AppState>) {
    if state.seated_on_ledge.borrow().is_some() {
        crate::stand_up_from_ledge(state).await;
    }
    if state.behavior.posture() != Posture::Sitting {
        return;
    }
    // WP-21 chair transitions (transition/sit_to_stand, idle) live in
    // behavior.py's run_sequence and are not ported; with no curated
    // tags the Python path also just flips the posture.
    state.behavior.set_posture(Posture::Standing);
}
