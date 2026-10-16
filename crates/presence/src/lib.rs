//! WP-10: hide/show/brb/summon - a small explicit state machine, so
//! felt-tray's hotkeys and felt-core's voice/text intents (both driving the
//! same felt_presence skill) act on one single source of truth for whether
//! she's currently on screen at all, rather than each guessing independently.
//!
//! Pure and clock-injectable (no async, no overlay calls) - the actual walk/
//! wave/visible-toggle orchestration lives in server.py, which drives this
//! state machine's transitions as each step of that orchestration completes.

/// On screen and fully idle.
pub const VISIBLE: &str = "visible";
/// Walk-off animation in progress (toward [`HIDDEN`] or [`AWAY`]).
pub const LEAVING: &str = "leaving";
/// Off screen indefinitely - no auto-return timer.
pub const HIDDEN: &str = "hidden";
/// Off screen with a running brb auto-return timer.
pub const AWAY: &str = "away";
/// Walk-in animation in progress.
pub const ARRIVING: &str = "arriving";

/// Mario's own ladder (2026-09-22): brb starts at 5 min; pressing it again
/// while already away climbs 5 -> 15 -> 30 -> 60, capping at 60.
pub const BRB_LADDER_MINUTES: [u32; 4] = [5, 15, 30, 60];

/// The hide/show/brb state machine. `now_fn` is the injectable monotonic
/// clock (seconds as f64; Python defaulted to `time.monotonic`).
pub struct PresenceState<C: Fn() -> f64> {
    now_fn: C,
    /// Current phase: one of [`VISIBLE`], [`LEAVING`], [`HIDDEN`], [`AWAY`],
    /// [`ARRIVING`]. Public and settable because the driving orchestration
    /// (and tests) poke it directly, exactly as in the Python original.
    pub state: &'static str,
    /// When the running brb timer expires, if any (monotonic seconds).
    away_until: Option<f64>,
    /// Position in [`BRB_LADDER_MINUTES`]; `None` = no brb pressed yet since
    /// the last show/hide (Python's `-1`).
    ladder_step: Option<usize>,
}

impl<C: Fn() -> f64> PresenceState<C> {
    /// Fresh state machine starting [`VISIBLE`].
    pub fn new(now_fn: C) -> Self {
        Self {
            now_fn,
            state: VISIBLE,
            away_until: None,
            ladder_step: None,
        }
    }

    /// True only when she's fully off screen ([`HIDDEN`] or [`AWAY`]) - not
    /// while a walk on/off screen is still in flight.
    pub fn is_offscreen(&self) -> bool {
        self.state == HIDDEN || self.state == AWAY
    }

    /// Seconds left on the brb timer; 0.0 when none is running.
    pub fn away_remaining_s(&self) -> f64 {
        let Some(away_until) = self.away_until else {
            return 0.0;
        };
        (away_until - (self.now_fn)()).max(0.0)
    }

    /// H: indefinite hide, no auto-return. Returns "walk_off" if she
    /// needs to START walking off screen, "already_leaving" if that walk
    /// is already in progress (a second press mid-walk must NOT fire a
    /// second concurrent walk-off animation - a real bug found live,
    /// 2026-09-23: pressing brb twice in quick succession, before the
    /// first walk-off had finished, fired two overlapping
    /// _walk_offscreen() tasks), or "already_off" if she's already
    /// hidden/away (just drops any running brb timer, no-op otherwise).
    pub fn request_hide(&mut self) -> &'static str {
        self.away_until = None;
        self.ladder_step = None;
        if self.is_offscreen() {
            self.state = HIDDEN;
            return "already_off";
        }
        if self.state == LEAVING {
            return "already_leaving";
        }
        self.state = LEAVING;
        "walk_off"
    }

    /// B: away with an auto-return timer. Returns ("walk_off"|
    /// "already_leaving"|"already_off", minutes). The ladder only climbs when
    /// she's already away (or already leaving toward away) - a fresh
    /// press while visible always restarts at the first rung, 5 minutes.
    /// See request_hide()'s docs for why "already_leaving" exists -
    /// the ladder still climbs on a mid-walk press, only the walk-off
    /// ANIMATION itself must not be re-fired.
    pub fn request_brb(&mut self) -> (&'static str, u32) {
        if (self.state == AWAY || self.state == LEAVING) && self.ladder_step.is_some() {
            let last = BRB_LADDER_MINUTES.len() - 1;
            self.ladder_step = Some((self.ladder_step.unwrap_or(0) + 1).min(last));
        } else {
            self.ladder_step = Some(0);
        }
        let minutes = BRB_LADDER_MINUTES[self.ladder_step.unwrap_or(0)];
        self.away_until = Some((self.now_fn)() + f64::from(minutes) * 60.0);
        if self.is_offscreen() {
            self.state = AWAY;
            return ("already_off", minutes);
        }
        if self.state == LEAVING {
            return ("already_leaving", minutes);
        }
        self.state = LEAVING;
        ("walk_off", minutes)
    }

    /// The walk-off animation actually finished - [`LEAVING`] becomes
    /// [`HIDDEN`] or [`AWAY`] depending on whether a brb timer is running.
    pub fn mark_offscreen(&mut self) {
        if self.state != LEAVING {
            return;
        }
        self.state = if self.away_until.is_some() {
            AWAY
        } else {
            HIDDEN
        };
    }

    /// show / summon / away-expiry. Returns false (no-op) if she's
    /// already visible or already on her way in.
    pub fn request_show(&mut self) -> bool {
        if self.state == VISIBLE || self.state == ARRIVING {
            return false;
        }
        self.state = ARRIVING;
        self.away_until = None;
        self.ladder_step = None;
        true
    }

    /// The walk-in animation actually finished - [`ARRIVING`] becomes
    /// [`VISIBLE`]; no-op otherwise.
    pub fn mark_onscreen(&mut self) {
        if self.state != ARRIVING {
            return;
        }
        self.state = VISIBLE;
    }

    /// True the first time an away timer is found expired - callers
    /// should immediately request_show() on a true result, since this
    /// also clears the timer so it only ever fires once.
    pub fn check_and_consume_expiry(&mut self) -> bool {
        if self.state != AWAY {
            return false;
        }
        let Some(away_until) = self.away_until else {
            return false;
        };
        if (self.now_fn)() >= away_until {
            self.away_until = None;
            return true;
        }
        false
    }
}

/// A roam range in world x: either a symmetric half-range (the old
/// primary-only frame) or WP-22's (lo, hi) world range across every monitor.
/// Mirrors Python's `float | tuple[float, float]` argument.
pub struct RoamSpan {
    pub lo: f64,
    pub hi: f64,
}

impl From<f64> for RoamSpan {
    /// Symmetric half-range `roam_limit` -> (-roam_limit, roam_limit).
    fn from(roam_limit: f64) -> Self {
        RoamSpan {
            lo: -roam_limit,
            hi: roam_limit,
        }
    }
}

impl From<(f64, f64)> for RoamSpan {
    /// WP-22's (lo, hi) world range across every monitor.
    fn from((lo, hi): (f64, f64)) -> Self {
        RoamSpan { lo, hi }
    }
}

/// Where "M" (shoo) sends her: at least `min_fraction` of the full
/// roam width away from `current_x`, preferring the end of the roam range
/// farther from the cursor.
/// - `roam_limit`: a half-range (symmetric, the old primary-only frame) or
///   WP-22's (lo, hi) world range across every monitor.
/// - `cursor_world_x`: the cursor's world x (WP-22, any monitor). Without
///   it, `cursor_fraction_x` (0.0 = left edge, 1.0 = right edge of the
///   range) stands in for it.
///
/// Pure/tested; never returns `current_x` itself when the roam range can
/// possibly avoid it, so repeated presses don't ever look like a
/// no-op/teleport-in-place.
pub fn pick_move_target(
    current_x: f64,
    roam_limit: impl Into<RoamSpan>,
    cursor_fraction_x: f64,
    min_fraction: f64,
    cursor_world_x: Option<f64>,
) -> f64 {
    let RoamSpan { lo, hi } = roam_limit.into();
    if hi - lo <= 0.0 {
        return current_x;
    }
    // Capped so a two-monitor range (14 m) still lets her shoo to the near end
    // of the primary (4 m away) instead of toward the cursor.
    let min_distance = (min_fraction * (hi - lo)).min(1.5);
    // Screen-right is world -x (main.gd's camera looks along +z).
    let cursor_world_x = cursor_world_x.unwrap_or(hi - cursor_fraction_x * (hi - lo));
    // Farther end from the cursor first; ties keep `hi` first, mirroring
    // Python's stable `sorted((hi, lo), key=lambda end: -abs(end - cursor_world_x))`.
    let (first, second) = if (hi - cursor_world_x).abs() >= (lo - cursor_world_x).abs() {
        (hi, lo)
    } else {
        (lo, hi)
    };
    for candidate in [first, second] {
        if (candidate - current_x).abs() >= min_distance {
            return candidate;
        }
    }
    first
}

/// Where "C" (come here) sends her: cursor_fraction_x (0.0-1.0 across
/// the virtual desktop) mapped onto [-roam_limit, roam_limit] and
/// clamped. An approximation (felt-animation doesn't know main.gd's real
/// camera/world mapping, just the same conservative roam range the
/// ambient wander loop already uses) - good enough for "walk toward
/// roughly where the cursor is," not a claim of pixel-perfect placement.
pub fn cursor_fraction_to_world_x(cursor_fraction_x: f64, roam_limit: f64) -> f64 {
    // Screen-right is world -x.
    let x = (0.5 - cursor_fraction_x) * 2.0 * roam_limit;
    x.min(roam_limit).max(-roam_limit)
}
