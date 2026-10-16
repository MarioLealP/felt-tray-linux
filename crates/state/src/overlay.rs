//! The slice of `felt_animation.overlay` that `felt_animation.state` needs,
//! defined here so this crate stands alone (see the port order: types from
//! other felt_animation modules are defined minimally in-crate rather than
//! ported). `LAYER_*` and `layer_for` mirror overlay.py; [OverlayLike] is
//! overlay.py's `OverlayLike` Protocol - the 4 `OverlayClient` methods
//! `BehaviorState` actually calls - as a trait so tests can pass a
//! lightweight double implementing just these.

use std::collections::HashMap;

// WP-20: main.gd's tag classes (its _tag_layer), reported over the relay as
// {"layers": {tag: class}} each time we connect. Godot decides them - which
// body part a clip plays on is presentation - and felt-animation only reads
// them to pick locks (state):
//   "upper" - layered over the body (gesture/emotion/reaction minus the
//             full-body overrides): can play while she walks or sits.
//   "base"  - postures/cycles an upper clip rides on (idle, locomotion ...).
//   "full"  - the whole body (transitions, ambient, dance, turn_surprised ...).
pub const LAYER_UPPER: &str = "upper";
pub const LAYER_BASE: &str = "base";
pub const LAYER_FULL: &str = "full";

/// The class main.gd reported for `tag`. A tag it didn't report (or an
/// older overlay that reports nothing) is treated as "full": the old
/// single-lock behaviour, never two clips fighting over the same bones.
#[must_use]
pub fn layer_for(tag: &str, reported: &HashMap<String, String>) -> String {
    reported
        .get(tag)
        .cloned()
        .unwrap_or_else(|| LAYER_FULL.to_string())
}

/// The 4 OverlayClient methods BehaviorState actually calls - a structural
/// Protocol (not the concrete OverlayClient, which owns a real TCP relay +
/// subprocess) so tests can pass a lightweight double implementing just
/// these. `tag_layer` is the deliberately-optional lookup of BehaviorState's
/// own `getattr`-based `layer_of()` (overlay.py's method when present):
/// returning `None` means "no layer report", like a pre-WP-20 overlay.
#[allow(async_fn_in_trait)] // structural test doubles only; never used as dyn
pub trait OverlayLike {
    async fn play_and_wait(&self, tag: &str, timeout: f64, clip: Option<&str>);
    async fn play_tag(&self, tag: &str, looped: bool);
    /// Python calls this `move`; renamed because `move` is a Rust keyword.
    async fn move_to(&self, x: f64, offscreen: bool, running: bool);
    async fn wait_for_arrived(&self, timeout: f64);

    /// `OverlayClient.tag_layer` - which body layer a tag plays on, from
    /// main.gd's {"layers"} report. `None` = this overlay has no such method
    /// (or reports nothing).
    fn tag_layer(&self, tag: &str) -> Option<String> {
        let _ = tag;
        None
    }
}
