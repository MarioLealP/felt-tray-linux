//! WP-23 slice 4: window events - riding a moving window, reacting to a
//! platform vanishing/maximising under her, and retreating from a fullscreen
//! app on her monitor.
//!
//! Pure logic here (fed Win/Monitor/Platform snapshots, unit tested); the
//! async orchestrators at the bottom are the thin glue loops's world tick
//! calls, doing real I/O only through [`Overlay`]/[`Behavior`] (fakeable in
//! tests, same convention as behavior/nav_executor).
//!
//! Mario's decisions this slice implements (WP-23 doc + 00-implementer-context
//! "reaction-first"/"never truly quiet"): window events ALWAYS get a
//! non-verbal reaction; any spoken line goes through the SAME budget the rest
//! of felt-animation's presence lines already use - `behavior.play_presence_line`
//! (a small curated-by-Mario wav pool per line, silently nothing if uncurated)
//! - plus a no-immediate-repeat pool and a cooldown here, never a fresh
//!   felt-core round trip for a canned line like this.

use std::collections::BTreeSet;
use std::future::Future;

// -- shared types, minimally ported from world.py ----------------------------
// (world.py is its own module/crate - only the definitions window_events
// actually touches live here.)

/// A (left, top, right, bottom) rect in desktop px.
pub type Rect = (f64, f64, f64, f64);

/// The foreground window: its hwnd plus its rect.
pub type Foreground = (isize, Rect);

/// One top-level window, desktop px. A list of these is meaningful
/// only in TOP-TO-BOTTOM z-order (index 0 = topmost).
#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    pub hwnd: isize,
    pub rect: Rect,
    pub title: String,
}

/// One monitor's bounds, desktop px. `work_bottom` is the work-area
/// bottom (excludes the taskbar) when known; falls back to `bottom`.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub index: i32,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub work_bottom: Option<f64>,
}

impl Monitor {
    /// The y a maximised window's bottom edge respects: the work-area
    /// bottom when known, the full monitor bottom otherwise.
    pub fn floor_y(&self) -> f64 {
        self.work_bottom.unwrap_or(self.bottom)
    }
}

/// One stand-on-able ledge, desktop px. `id` is stable across ticks
/// for the same window/monitor/seam so the client can tell
/// "she's still on the same platform" from "the platform moved".
/// `kind` is "floor" | "window" | "seam".
#[derive(Debug, Clone, PartialEq)]
pub struct Platform {
    pub id: String,
    pub hwnd: Option<isize>,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    pub kind: String,
}

/// A window/seam rect within this many px of its monitor's bounds on every
/// edge counts as covering the whole monitor (maximised/fullscreen) - the
/// tolerance world.py uses for its own maximised/fullscreen classification,
/// reused here rather than duplicated.
pub const FULLSCREEN_TOLERANCE_PX: f64 = 4.0;

/// Whether `rect` covers `monitor`'s full bounds (taskbar included), within
/// `tol` px on every edge.
pub fn covers_whole_monitor_tol(rect: Rect, monitor: &Monitor, tol: f64) -> bool {
    let (left, top, right, bottom) = rect;
    left <= monitor.left + tol
        && right >= monitor.right - tol
        && top <= monitor.top + tol
        && bottom >= monitor.floor_y() - tol
}

/// [`covers_whole_monitor_tol`] with world.py's default tolerance.
pub fn covers_whole_monitor(rect: Rect, monitor: &Monitor) -> bool {
    covers_whole_monitor_tol(rect, monitor, FULLSCREEN_TOLERANCE_PX)
}

/// The monitor containing (cx, cy), if any. `left`/`top` inclusive,
/// `right`/`bottom` exclusive - same as world.py.
pub fn monitor_for_point(cx: f64, cy: f64, monitors: &[Monitor]) -> Option<&Monitor> {
    monitors
        .iter()
        .find(|m| m.left <= cx && cx < m.right && m.top <= cy && cy < m.bottom)
}

// -- shared type, minimally ported from randomness.py ------------------------

/// `random.RandomLike`'s structural interface, reduced to the one call
/// window_events makes: pick one element of a non-empty sequence. The
/// element's index is returned so the trait stays object-safe and free of
/// generics; implementations wrap whatever RNG is on hand.
pub trait RandomLike {
    /// `rng.choice(seq)` - one index in `0..len` (`len` is never 0 at any
    /// call site in this crate).
    fn choice_index(&mut self, len: usize) -> usize;
}

// -- shared helper, minimally ported from library.py -------------------------

/// library.resolve_known_tag: felt-core can't know the desktop's animation
/// tags, so any tag has to be checked against what's actually known before
/// it's trusted. Exact match wins; otherwise walk up the path
/// ("a/b/c" -> "a/b" -> "a") to the nearest ancestor that's a real tag,
/// since a broader real tag is still a better reaction than nothing. None
/// if nothing along that chain matches, or if no tag was supplied at all.
pub fn resolve_known_tag(tag: &str, known_tags: &BTreeSet<String>) -> Option<String> {
    if tag.is_empty() {
        return None;
    }
    if known_tags.contains(tag) {
        return Some(tag.to_string());
    }
    let parts: Vec<&str> = tag.split('/').collect();
    for i in (1..parts.len()).rev() {
        let candidate = parts[..i].join("/");
        if known_tags.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

// -- collaborator ports (real impls live in the behavior/navigation crates) --

/// One message for the main.gd overlay - the only one window_events sends:
/// `{"shift": {"dx_px": dx_px, "y_px": y_px}}`.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayMessage {
    /// Carry her along with a window she's riding this tick.
    Shift { dx_px: f64, y_px: f64 },
}

/// `state.overlay` - delivery to the overlay client. `send` is false if the
/// message couldn't be delivered; the caller treats that as "nothing
/// happened".
pub trait Overlay {
    fn send(&mut self, message: OverlayMessage) -> impl Future<Output = bool> + Send;
}

/// `state.geometry()` - her px<->world mapping plus pixels-per-meter.
pub trait Geometry {
    fn world_to_px_x(&self, world_x: f64) -> f64;
    fn px_to_world_x(&self, x_px: f64) -> f64;
    fn ppm(&self) -> f64;
}

/// `time.monotonic` as an injectable clock, so the speech cooldown is
/// deterministic under test.
pub trait Clock {
    fn monotonic(&self) -> f64;
}

/// Opaque navigation plan - navigation.py's `list[Step]` (its own module).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan(pub Vec<String>);

/// The side effects window_events drives through behavior.py/event_log.py/
/// nav_executor.py. `run_sequence`/`run_plan` are fire-and-forget in the
/// Python (`asyncio.create_task(...)`): the port starts them and returns,
/// so the methods are not async - starting/spawning is the impl's job.
pub trait Behavior {
    /// `asyncio.create_task(behavior.run_sequence(state, tags))` - play an
    /// animation tag sequence, fire-and-forget.
    fn start_run_sequence(&mut self, tags: Vec<String>);
    /// `behavior.play_presence_line(state, line)` - True if a curated wav
    /// pool for this line exists and started playing (silently nothing
    /// otherwise).
    fn play_presence_line(&mut self, line: &str) -> bool;
    /// `event_log.report_event(state, kind, data)` - fire-and-forget POST,
    /// no-op when reporting is disabled or unconfigured.
    fn report_event(&mut self, kind: &str, data: &[(&str, &str)]);
}

/// navigation.py's `plan_route` + nav_executor.py's `run_plan`, as one port
/// (both modules are their own crates).
pub trait Navigator {
    /// `navigation.plan_route(...)` - None if the target platform isn't in
    /// `platforms` or there's no path.
    fn plan_route(
        &self,
        platforms: &[Platform],
        ppm: f64,
        current_platform_id: &str,
        current_x_px: f64,
        target_platform_id: &str,
        target_x_px: f64,
    ) -> Option<Plan>;
    /// `asyncio.create_task(nav_executor.run_plan(state, plan))` - walk a
    /// plan, fire-and-forget.
    fn start_run_plan(&mut self, plan: Plan);
}

/// The slice of runtime.AppState window_events reads and mutates. (AppState
/// itself is runtime.py's - only these fields are this module's business.)
#[derive(Debug, Clone, Default)]
pub struct WindowEventState {
    /// The platform id she's anchored to (None until the first tick).
    pub current_platform_id: Option<String>,
    /// Her x in world units.
    pub current_x: f64,
    /// Fullscreen-retreat episode latch: set once a retreat is planned,
    /// cleared when the fullscreen episode ends.
    pub fullscreen_retreat_active: bool,
    /// Animation tags known to exist on this machine (library.resolve_known_tag).
    pub known_tags: BTreeSet<String>,
    /// `config.window_event_speech_enabled` - the spoken-line budget switch.
    pub window_event_speech_enabled: bool,
    /// No-immediate-repeat pool bookkeeping: the annoyed line played last.
    pub window_event_last_line: Option<String>,
    /// Speech cooldown bookkeeping: `clock.monotonic()` of the last line.
    pub window_event_last_spoken_at: f64,
}

// -- constants ---------------------------------------------------------------

/// "her platform vanished under her" pool - rotated with no immediate
/// repeat by pick_annoyed_line. Each name is a behavior.play_presence_line
/// line id (sounds_dir/presence/<name>_*.wav) - "rude" already exists from
/// WP-19; "standing_there"/"wow_okay" are new ids for the WP's own example
/// lines ("Hey! I was standing there.", "Wow. Okay.") - uncurated until
/// Mario records/keeps takes for them (sounds.py degrades silently either
/// way, same as every other presence line).
pub const WINDOW_ANNOYED_LINES: [&str; 3] = ["rude", "standing_there", "wow_okay"];
/// A real fall-cause event (a window closing/minimising/moving away under
/// her) is already rare - this cooldown just guards against a burst of them
/// (e.g. a window flickering) turning into a burst of lines.
pub const WINDOW_EVENT_SPEECH_COOLDOWN_S: f64 = 45.0;
/// "Retreat to a screen corner" - how far in from the floor's own edge, so
/// she doesn't stand exactly at the world boundary.
pub const RETREAT_CORNER_MARGIN_PX: f64 = 80.0;

// -- pure logic --------------------------------------------------------------

fn center(rect: Rect) -> (f64, f64) {
    let (left, top, right, bottom) = rect;
    ((left + right) / 2.0, (top + bottom) / 2.0)
}

/// None unless she's standing on a window-kind platform whose hwnd is
/// present in both window snapshots with a moved rect (its top y and/or
/// left x shifted), AND her x, shifted by that same dx, still lands
/// inside the window's new real x-range (its own rect, not the
/// possibly-occlusion-clipped platform segment - occlusion is a separate,
/// later concern the ordinary support sync still catches on its own next
/// tick). Returns (dx_px, new_top_y_px) for the {"shift"} message;
/// otherwise she's ridden off the end and the caller falls through to the
/// ordinary vanish/fall handling.
pub fn detect_ride(
    current_platform: Option<&Platform>,
    previous_windows: &[Win],
    windows: &[Win],
    her_x_px: f64,
) -> Option<(f64, f64)> {
    let current_platform = current_platform?;
    if current_platform.kind != "window" {
        return None;
    }
    let hwnd = current_platform.hwnd?;
    let old_win = previous_windows.iter().find(|w| w.hwnd == hwnd)?;
    let new_win = windows.iter().find(|w| w.hwnd == hwnd)?;
    let (old_left, old_top, _old_right, _old_bottom) = old_win.rect;
    let (new_left, new_top, new_right, _new_bottom) = new_win.rect;
    let dx = new_left - old_left;
    let dy = new_top - old_top;
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    let new_x = her_x_px + dx;
    let lands_in_new_range = new_left <= new_x && new_x <= new_right;
    if !lands_in_new_range {
        return None;
    }
    Some((dx, new_top))
}

/// Why the platform she was standing on is gone, for react_to_landing's
/// reaction/speech choice - "vanished" (closed or minimised: hwnd absent
/// from the window enumeration either way), "maximised" (hwnd still there,
/// now covering its monitor's work area, same check compute_platforms uses
/// to drop it as a platform in the first place), or "moved_away" (hwnd
/// still there, somewhere else - covers a window that moved out from under
/// her, or one that's simply become occluded where she stood).
pub fn classify_fall_cause(
    last_platform: &Platform,
    windows: &[Win],
    monitors: &[Monitor],
) -> &'static str {
    if last_platform.kind != "window" {
        return "vanished";
    }
    let Some(hwnd) = last_platform.hwnd else {
        return "vanished";
    };
    let Some(win) = windows.iter().find(|w| w.hwnd == hwnd) else {
        return "vanished";
    };
    let (cx, cy) = center(win.rect);
    match monitor_for_point(cx, cy, monitors) {
        Some(monitor) if covers_whole_monitor(win.rect, monitor) => "maximised",
        _ => "moved_away",
    }
}

/// The foreground window covers `monitor`'s FULL bounds, taskbar
/// included (monitor.bottom, not the work-area floor_y a maximised window
/// respects) - same idea as capture_win32's
/// `_is_fullscreen_on_its_monitor`, but against the real foreground window
/// rather than every enumerated window (the WP's own "foreground" bullet).
pub fn is_fullscreen_on_monitor(foreground: Option<Foreground>, monitor: &Monitor) -> bool {
    is_fullscreen_on_monitor_tol(foreground, monitor, FULLSCREEN_TOLERANCE_PX)
}

/// [`is_fullscreen_on_monitor`] with an explicit edge tolerance.
pub fn is_fullscreen_on_monitor_tol(
    foreground: Option<Foreground>,
    monitor: &Monitor,
    tol: f64,
) -> bool {
    let Some((_hwnd, rect)) = foreground else {
        return false;
    };
    let (left, top, right, bottom) = rect;
    left <= monitor.left + tol
        && right >= monitor.right - tol
        && top <= monitor.top + tol
        && bottom >= monitor.bottom - tol
}

/// A corner of another monitor's floor - "retreat to a screen corner of
/// the OTHER monitor" (the WP's own wording). None with only one monitor,
/// or if that monitor's floor somehow isn't in `platforms` this tick.
/// Returns (that floor, the corner's x).
pub fn pick_retreat_target<'a>(
    platforms: &'a [Platform],
    monitors: &[Monitor],
    her_monitor_index: i32,
) -> Option<(&'a Platform, f64)> {
    let target_monitor = monitors.iter().find(|m| m.index != her_monitor_index)?;
    let index = target_monitor.index;
    let floor_prefix = format!("floor:{index}");
    let floor = platforms
        .iter()
        .find(|p| p.kind == "floor" && p.id.starts_with(&floor_prefix))?;
    let margin = RETREAT_CORNER_MARGIN_PX.min((floor.x1 - floor.x0) / 2.0);
    Some((floor, floor.x0 + margin))
}

/// One line from the vanished-under-her pool, never repeating `last_line`.
/// Falls back to the whole pool when everything is excluded (can't happen
/// with a one-line history and a 3-line pool, but cheap to be exact).
pub fn pick_annoyed_line<R: RandomLike + ?Sized>(
    last_line: Option<&str>,
    rng: &mut R,
) -> &'static str {
    let candidates: Vec<&'static str> = WINDOW_ANNOYED_LINES
        .iter()
        .copied()
        .filter(|line| Some(*line) != last_line)
        .collect();
    let candidates = if candidates.is_empty() {
        WINDOW_ANNOYED_LINES.to_vec()
    } else {
        candidates
    };
    candidates[rng.choice_index(candidates.len())]
}

// -- async orchestrators (the world tick calls these) ------------------------

/// True if she was carried this tick (a {"shift"} was sent) - the
/// caller (the world tick) then skips the ordinary support sync for
/// this tick, since riding is not a fall. Skipped while `base_busy` (an
/// active walk/fall/hop/climb already owns her), same guard the support
/// sync uses.
pub async fn try_ride<O: Overlay, G: Geometry>(
    state: &mut WindowEventState,
    overlay: &mut O,
    geometry: &G,
    base_busy: bool,
    previous_platforms: &[Platform],
    previous_windows: &[Win],
    windows: &[Win],
) -> bool {
    if state.current_platform_id.is_none() || base_busy {
        return false;
    }
    let current = previous_platforms
        .iter()
        .find(|p| Some(p.id.as_str()) == state.current_platform_id.as_deref());
    let her_x_px = geometry.world_to_px_x(state.current_x);
    let Some((dx_px, new_top_y)) = detect_ride(current, previous_windows, windows, her_x_px) else {
        return false;
    };
    let sent = overlay
        .send(OverlayMessage::Shift {
            dx_px,
            y_px: new_top_y,
        })
        .await;
    if !sent {
        return false;
    }
    state.current_x = geometry.px_to_world_x(her_x_px + dx_px);
    true
}

/// Fired once from AppState's on-world-settled hook on a real {"landed"}
/// whose fall was recorded with a cause (the support sync only records one
/// for an AUTOMATIC platform vanish - a deliberate nav hop/climb never
/// sets one, so this never fires for those). Non-verbal always; speech
/// only through `behavior.play_presence_line`, budgeted (cooldown + no-
/// immediate-repeat pool), and only for "vanished"/"moved_away" - a
/// maximise is a surprise, not rudeness, so it stays non-verbal only (a
/// judgment call, not Mario's own wording).
pub async fn react_to_landing<C: Clock, R: RandomLike + ?Sized, B: Behavior>(
    state: &mut WindowEventState,
    cause: &str,
    clock: &C,
    rng: &mut R,
    behavior: &mut B,
) {
    behavior.report_event("window_fall", &[("cause", cause)]);
    let tag_name = if cause == "maximised" {
        "emotion/surprised"
    } else {
        "emotion/annoyed"
    };
    if let Some(tag) = resolve_known_tag(tag_name, &state.known_tags) {
        behavior.start_run_sequence(vec![tag]);
    }
    // else: no known tag for this reaction - skip the animation, same as
    // Python's logger.warning fall-through (nothing else to do).

    if cause == "maximised" || !state.window_event_speech_enabled {
        return;
    }
    let now = clock.monotonic();
    if now - state.window_event_last_spoken_at < WINDOW_EVENT_SPEECH_COOLDOWN_S {
        return;
    }
    let line = pick_annoyed_line(state.window_event_last_line.as_deref(), rng);
    if behavior.play_presence_line(line) {
        state.window_event_last_line = Some(line.to_string());
        state.window_event_last_spoken_at = now;
    }
}

/// Item 4: a fullscreen app on her own monitor -> walk to a corner of
/// the OTHER monitor's floor, once per episode (`fullscreen_retreat_active`
/// latches until fullscreen ends, so this doesn't replan every tick while
/// it's still on). No-op with one monitor, or while she isn't anchored to
/// a real platform yet.
pub async fn maybe_retreat_from_fullscreen<G: Geometry, N: Navigator>(
    state: &mut WindowEventState,
    geometry: &G,
    base_busy: bool,
    nav: &mut N,
    platforms: &[Platform],
    monitors: &[Monitor],
    foreground: Option<Foreground>,
) {
    if monitors.len() < 2 {
        return;
    }
    let Some(current_id) = state.current_platform_id.as_deref() else {
        return;
    };
    let Some(current_platform) = platforms.iter().find(|p| p.id == current_id) else {
        return;
    };
    let her_x_px = geometry.world_to_px_x(state.current_x);
    let Some(her_monitor) = monitor_for_point(her_x_px, current_platform.y - 1.0, monitors) else {
        return;
    };

    if !is_fullscreen_on_monitor(foreground, her_monitor) {
        state.fullscreen_retreat_active = false;
        return;
    }
    if state.fullscreen_retreat_active || base_busy {
        return;
    }

    let target = pick_retreat_target(platforms, monitors, her_monitor.index);
    let Some((floor, corner_x)) = target else {
        return;
    };
    let ppm = if geometry.ppm() > 0.0 {
        geometry.ppm()
    } else {
        1.0
    };
    let Some(plan) = nav.plan_route(platforms, ppm, current_id, her_x_px, &floor.id, corner_x)
    else {
        return;
    };
    state.fullscreen_retreat_active = true;
    nav.start_run_plan(plan);
}
