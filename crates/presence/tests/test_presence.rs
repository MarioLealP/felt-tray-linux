//! PresenceState (WP-10): hide/show/brb transitions, the brb ladder, and
//! the move/come target pickers. Clock-injected, no real async/overlay
//! involved - server.py's own orchestration (walk/wave/visible-toggle) is
//! exercised separately.

use felt_presence::{
    ARRIVING, AWAY, HIDDEN, LEAVING, PresenceState, VISIBLE, cursor_fraction_to_world_x,
    pick_move_target,
};
use std::cell::Cell;
use std::rc::Rc;

/// Python's `_clock(box)`: a closure over a mutable box of monotonic seconds
/// (named `time_box` here - `box` is a Rust keyword).
fn clock(time_box: Rc<Cell<f64>>) -> impl Fn() -> f64 {
    move || time_box.get()
}

// -- hide -------------------------------------------------------------

#[test]
fn test_hide_from_visible_walks_off() {
    let mut p = PresenceState::new(clock(Rc::new(Cell::new(0.0))));
    assert_eq!(p.request_hide(), "walk_off");
    assert_eq!(p.state, LEAVING);
}

#[test]
fn test_hide_when_already_hidden_is_a_noop() {
    let mut p = PresenceState::new(clock(Rc::new(Cell::new(0.0))));
    p.request_hide();
    p.mark_offscreen();
    assert_eq!(p.state, HIDDEN);
    assert_eq!(p.request_hide(), "already_off");
    assert_eq!(p.state, HIDDEN);
}

#[test]
fn test_hide_drops_a_running_brb_timer() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_brb();
    p.mark_offscreen();
    assert_eq!(p.state, AWAY);
    p.request_hide();
    assert_eq!(p.state, HIDDEN);
    assert_eq!(p.away_remaining_s(), 0.0);
}

// -- brb / the ladder ---------------------------------------------------

#[test]
fn test_brb_from_visible_starts_at_five_minutes() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    let (outcome, minutes) = p.request_brb();
    assert_eq!(outcome, "walk_off");
    assert_eq!(minutes, 5);
    assert_eq!(p.state, LEAVING);
    assert!((p.away_remaining_s() - 5.0 * 60.0).abs() < 1e-6);
}

#[test]
fn test_brb_pressed_again_while_away_climbs_the_ladder() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_brb();
    p.mark_offscreen();
    assert_eq!(p.state, AWAY);

    let (_, minutes) = p.request_brb();
    assert_eq!(minutes, 15);
    let (_, minutes) = p.request_brb();
    assert_eq!(minutes, 30);
    let (_, minutes) = p.request_brb();
    assert_eq!(minutes, 60);
    let (_, minutes) = p.request_brb(); // caps, doesn't overflow the ladder
    assert_eq!(minutes, 60);
}

#[test]
fn test_brb_pressed_while_already_offscreen_skips_the_walk() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_brb();
    p.mark_offscreen();
    let (outcome, _) = p.request_brb();
    assert_eq!(outcome, "already_off");
    assert_eq!(p.state, AWAY);
}

#[test]
fn test_brb_pressed_again_mid_walk_climbs_the_ladder_without_a_second_walk() {
    // Real bug found live, 2026-09-23: pressing B twice before the first
    // walk-off animation had finished fired two overlapping walk-off
    // tasks. The ladder must still climb, but the caller must be told NOT
    // to start a second walk.
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    let (outcome1, minutes1) = p.request_brb();
    assert_eq!(outcome1, "walk_off");
    assert_eq!(minutes1, 5);
    assert_eq!(p.state, LEAVING); // still mid-walk, mark_offscreen() not called yet

    let (outcome2, minutes2) = p.request_brb();
    assert_eq!(outcome2, "already_leaving"); // NOT "walk_off" - no second walk
    assert_eq!(minutes2, 15); // ladder still climbed
    assert_eq!(p.state, LEAVING);
}

#[test]
fn test_hide_pressed_again_mid_walk_does_not_refire_the_walk() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    assert_eq!(p.request_hide(), "walk_off");
    assert_eq!(p.request_hide(), "already_leaving");
    assert_eq!(p.state, LEAVING);
}

#[test]
fn test_brb_while_visible_after_a_prior_completed_away_restarts_the_ladder() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_brb();
    p.mark_offscreen();
    p.request_show();
    p.mark_onscreen();
    assert_eq!(p.state, VISIBLE);

    let (_, minutes) = p.request_brb();
    assert_eq!(minutes, 5); // fresh ladder, not continuing from last time
}

// -- show / summon --------------------------------------------------------

#[test]
fn test_show_from_hidden_arrives() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_hide();
    p.mark_offscreen();
    assert!(p.request_show());
    assert_eq!(p.state, ARRIVING);
    p.mark_onscreen();
    assert_eq!(p.state, VISIBLE);
}

#[test]
fn test_show_while_already_visible_is_a_noop() {
    let mut p = PresenceState::new(clock(Rc::new(Cell::new(0.0))));
    assert!(!p.request_show());
    assert_eq!(p.state, VISIBLE);
}

#[test]
fn test_show_while_already_arriving_is_a_noop() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_hide();
    p.mark_offscreen();
    p.request_show();
    assert!(!p.request_show());
    assert_eq!(p.state, ARRIVING);
}

// -- away expiry ----------------------------------------------------------

#[test]
fn test_expiry_not_yet_reached() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_brb();
    p.mark_offscreen();
    time_box.set(100.0);
    assert!(!p.check_and_consume_expiry());
}

#[test]
fn test_expiry_reached_fires_exactly_once() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_brb();
    p.mark_offscreen();
    time_box.set(5.0 * 60.0 + 1.0);
    assert!(p.check_and_consume_expiry());
    assert!(!p.check_and_consume_expiry()); // consumed, doesn't refire
}

#[test]
fn test_expiry_never_fires_for_indefinite_hide() {
    let time_box = Rc::new(Cell::new(0.0));
    let mut p = PresenceState::new(clock(Rc::clone(&time_box)));
    p.request_hide();
    p.mark_offscreen();
    time_box.set(10f64.powi(6));
    assert!(!p.check_and_consume_expiry());
}

// -- pick_move_target -------------------------------------------------

#[test]
fn test_move_prefers_side_away_from_cursor_on_the_left() {
    // cursor on the screen's left -> go to screen right = world -x
    let target = pick_move_target(0.0, 2.0, 0.1, 0.3, None);
    assert_eq!(target, -2.0);
}

#[test]
fn test_move_prefers_side_away_from_cursor_on_the_right() {
    // cursor on the screen's right -> go to screen left = world +x
    let target = pick_move_target(0.0, 2.0, 0.9, 0.3, None);
    assert_eq!(target, 2.0);
}

#[test]
fn test_move_never_returns_current_position_when_avoidable() {
    // already sitting at the preferred edge - must pick the OTHER edge
    // rather than a same-spot "move"
    let target = pick_move_target(-2.0, 2.0, 0.1, 0.3, None);
    assert_eq!(target, 2.0);
    assert_ne!(target, -2.0);
}

#[test]
fn test_move_target_is_at_least_thirty_percent_of_roam_width_away() {
    let target = pick_move_target(0.5, 2.0, 0.9, 0.3, None);
    assert!((target - 0.5).abs() >= 0.3 * 4.0);
}

#[test]
fn test_move_with_zero_roam_limit_is_a_noop() {
    assert_eq!(pick_move_target(0.3, 0.0, 0.5, 0.3, None), 0.3);
}

// -- cursor_fraction_to_world_x -------------------------------------------

#[test]
fn test_cursor_center_maps_to_world_center() {
    assert_eq!(cursor_fraction_to_world_x(0.5, 2.0), 0.0);
}

#[test]
fn test_cursor_left_edge_maps_to_negative_roam_limit() {
    // screen-left is world +x
    assert_eq!(cursor_fraction_to_world_x(0.0, 2.0), 2.0);
}

#[test]
fn test_cursor_right_edge_maps_to_positive_roam_limit() {
    assert_eq!(cursor_fraction_to_world_x(1.0, 2.0), -2.0);
}

#[test]
fn test_cursor_mapping_is_clamped_even_if_fraction_is_out_of_range() {
    assert_eq!(cursor_fraction_to_world_x(1.5, 2.0), -2.0);
    assert_eq!(cursor_fraction_to_world_x(-0.5, 2.0), 2.0);
}
