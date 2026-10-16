//! WP-14 gaze decisions (audit 2026-09-24): felt-animation decides where she
//! looks and tells main.gd, instead of main.gd deciding "cursor still 4 s ->
//! camera" and "speaking -> camera" itself (Godot is presentation only).
//!
//! Relay contract with main.gd: `{"gaze": {"mode": "cursor" | "camera" | "off"}}`
//! plus the pre-existing `{"gaze": {"mode": "point", "x": fx, "y": fy}}` used for
//! the thinking glance (viewport fractions). Sent only when the decision
//! changes, and resent whenever the overlay (re)connects.
//!
//! Priority, highest first:
//!   1. off     - a tag on the per-tag opt-out list is playing (clips where
//!      additive head tracking would fight the clip's own head/body
//!      motion: falls, climbs, get-ups, nods, dances ...)
//!   2. camera  - she is speaking (reply playback start -> end)
//!   3. point   - she is thinking (up-left glance)
//!   4. camera  - the cursor has been still for >= 4 s
//!   5. cursor  - otherwise

/// Cursor must be still this long before she looks at the camera.
pub const CURSOR_STILL_TO_CAMERA_S: f64 = 4.0;

/// Up-left glance position for the thinking "point" mode (viewport fractions).
pub const THINKING_POINT: (f64, f64) = (0.35, 0.3);

/// WP-14 "a per-tag opt-out list in felt-animation": tags (or tag prefixes -
/// "air" covers "air/fall") during which head tracking is switched off.
pub const DEFAULT_OPT_OUT_TAGS: &[&str] = &[
    "air",
    "climb",
    "recover",
    "crouch",
    "prop",
    "dance",
    "sleepy",
    "locomotion/turn",
    "reaction/turn_surprised",
    "gesture/nodding",
    "gesture/shaking_head",
];

/// Gaze mode as sent over the relay (the `mode` field of the `gaze` message).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GazeMode {
    /// Head tracking off - an opt-out clip is playing.
    Off,
    /// Look at the camera (speaking, or cursor still for >= 4 s).
    Camera,
    /// Thinking glance at [`THINKING_POINT`].
    Point,
    /// Follow the cursor.
    Cursor,
}

impl GazeMode {
    /// The wire value for the relay contract (`"off" | "camera" | "point" | "cursor"`).
    pub fn as_str(self) -> &'static str {
        match self {
            GazeMode::Off => "off",
            GazeMode::Camera => "camera",
            GazeMode::Point => "point",
            GazeMode::Cursor => "cursor",
        }
    }
}

/// `(mode, x, y)` - x/y only for "point".
pub type GazeDecision = (GazeMode, Option<f64>, Option<f64>);

/// True if `tag` is on (or under) the per-tag opt-out list.
///
/// Matches a tag exactly or per path segment prefix: "air" covers "air/fall"
/// but not "airplane". A missing/empty tag is never opted out.
pub fn is_opted_out(tag: Option<&str>, opt_out: &[&str]) -> bool {
    match tag {
        None | Some("") => false,
        Some(tag) => opt_out.iter().any(|prefix| {
            tag == *prefix
                || tag
                    .strip_prefix(*prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        }),
    }
}

/// Pure: (mode, x, y) - x/y only for "point".
pub fn decide_gaze(
    opted_out: bool,
    speaking: bool,
    thinking: bool,
    cursor_still_s: f64,
) -> GazeDecision {
    if opted_out {
        (GazeMode::Off, None, None)
    } else if speaking {
        (GazeMode::Camera, None, None)
    } else if thinking {
        (
            GazeMode::Point,
            Some(THINKING_POINT.0),
            Some(THINKING_POINT.1),
        )
    } else if cursor_still_s >= CURSOR_STILL_TO_CAMERA_S {
        (GazeMode::Camera, None, None)
    } else {
        (GazeMode::Cursor, None, None)
    }
}

/// Injectable clock so tests are deterministic (Python: `time.monotonic`).
pub trait Clock {
    /// Current time in seconds.
    fn now(&self) -> f64;
}

impl<F: Fn() -> f64> Clock for F {
    fn now(&self) -> f64 {
        self()
    }
}

/// Holds the inputs, sends the decision on change. `send(mode, x, y)`
/// returns `Some(true)`/`None` if the overlay actually got it (`Some(false)`
/// -> retried on the next update, and on reconnect via [`GazeController::resend`]).
pub struct GazeController<'a, C: Clock, S> {
    send: S,
    opt_out: &'a [&'a str],
    now_fn: C,
    pub speaking: bool,
    pub thinking: bool,
    pub current_tag: Option<String>,
    cursor_pos: Option<(i32, i32)>,
    cursor_moved_at: f64,
    sent: Option<GazeDecision>,
}

impl<'a, C: Clock, S> GazeController<'a, C, S>
where
    S: FnMut(GazeMode, Option<f64>, Option<f64>) -> Option<bool>,
{
    /// Creates a controller with [`DEFAULT_OPT_OUT_TAGS`].
    pub fn new(send: S, now_fn: C) -> Self {
        Self::with_opt_out(send, DEFAULT_OPT_OUT_TAGS, now_fn)
    }

    /// Creates a controller with a custom per-tag opt-out list.
    pub fn with_opt_out(send: S, opt_out: &'a [&'a str], now_fn: C) -> Self {
        let cursor_moved_at = now_fn.now();
        Self {
            send,
            opt_out,
            now_fn,
            speaking: false,
            thinking: false,
            current_tag: None,
            cursor_pos: None,
            cursor_moved_at,
            sent: None,
        }
    }

    /// Records the latest cursor position; a moved cursor resets the still timer.
    pub fn observe_cursor(&mut self, pos: Option<(i32, i32)>) {
        let Some(pos) = pos else {
            return;
        };
        if Some(pos) != self.cursor_pos {
            self.cursor_pos = Some(pos);
            self.cursor_moved_at = self.now_fn.now();
        }
    }

    /// The current (mode, x, y) from the inputs held so far.
    pub fn decision(&self) -> GazeDecision {
        decide_gaze(
            is_opted_out(self.current_tag.as_deref(), self.opt_out),
            self.speaking,
            self.thinking,
            self.now_fn.now() - self.cursor_moved_at,
        )
    }

    /// Sends the decision to the overlay if it changed since the last send.
    pub fn update(&mut self) {
        let wanted = self.decision();
        if Some(wanted) == self.sent {
            return;
        }
        let delivered = (self.send)(wanted.0, wanted.1, wanted.2);
        if delivered == Some(false) {
            self.sent = None; // try again next update / on reconnect
            return;
        }
        self.sent = Some(wanted);
    }

    /// The overlay (re)connected - it knows nothing yet.
    pub fn resend(&mut self) {
        self.sent = None;
        self.update();
    }
}
