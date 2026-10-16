//! The "never interrupt, always finish the current thing first" rule
//! (Mario, 2026-09-21) applies uniformly to two situations: ambient behavior
//! being interrupted by an incoming request, and a thinking animation being
//! cut short by an already-ready reply. Both are implemented here as the same
//! underlying mechanism - a single activity lock that any one-shot sequence
//! (an ambient walk, the reply's reaction) holds for its whole duration, plus
//! a thinking loop that keeps re-playing a tag until told to stop, but only
//! actually stops between iterations (never mid-clip), and itself waits its
//! turn for the same lock before it ever starts.
//!
//! WP-20 (AnimationTree layering) splits that lock in two, one per body layer
//! in main.gd: the base lock (legs/hips/posture: walks, idles, transitions)
//! and the upper lock (the upper-body layer: gestures, emotions, reactions).
//! The class of each tag comes from main.gd's {"layers"} report
//! (`overlay::layer_for`):
//!   - "upper" tags take only the upper lock, so a reply gesture, a reaction,
//!     the thinking loop or an emotion plays WHILE she walks or sits;
//!   - every other tag takes both (base first, then upper - always that order,
//!     so two sequences can't deadlock), which is exactly the old single lock:
//!     a full-body clip still waits for, and never cuts off, a gesture;
//!   - walk_to takes only the base lock - a walk is what a gesture rides on.
//!
//! The rule itself is unchanged, per layer: nothing interrupts what is already
//! playing on the body part it needs.
//!
//! This is the Rust port of felt_animation/state.py. The `asyncio` machinery
//! it stands on (locks, events, tasks, timers) lives in [exec]; the slice of
//! felt_animation.overlay it needs lives in [overlay].

pub mod exec;
pub mod overlay;
mod state;

pub use overlay::{LAYER_BASE, LAYER_FULL, LAYER_UPPER, OverlayLike, layer_for};
pub use state::{BehaviorState, Clock, MonotonicClock, layer_of};
