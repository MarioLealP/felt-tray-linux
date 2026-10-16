//! WP-23 slice 1: the desktop "world model" - pure functions, no Win32 (see
//! windows_win32.py for the real adapter). Turns a z-ordered window list plus
//! the monitor layout into a list of stand-on-able platforms: window top
//! edges (minus whatever's occluding them), one floor per monitor, and any
//! fixed background ledges (`extra_seams` - e.g. Mario's DISPLAY2 2x2 grid
//! split).
//!
//! This slice changes NO movement and sends NOTHING to the overlay - it only
//! computes and caches (loops.py) and exposes GET /world for debugging.

/// A platform segment narrower than this is dropped - too thin to stand on.
pub const MIN_SEGMENT_WIDTH_PX: f64 = 120.0;
/// "keep a segment only if y - monitor_top >= sit clearance" - the WP's own
/// number: 0.6 * her standing height (enough headroom to sit, not stand).
pub const SIT_CLEARANCE_FACTOR: f64 = 0.6;
/// A window/seam rect within this many px of its monitor's bounds on every
/// edge counts as covering the whole monitor (maximised/fullscreen) - same
/// tolerance felt-tray's capture_win32._is_fullscreen_on_its_monitor uses.
/// Public (WP-23 slice 4's window_events.py reuses it for its own maximised/
/// fullscreen classification rather than duplicating the tolerance check).
pub const FULLSCREEN_TOLERANCE_PX: f64 = 4.0;

/// One top-level window, desktop px. A list of these is meaningful
/// only in TOP-TO-BOTTOM z-order (index 0 = topmost) - see
/// windows_win32.enumerate_windows_z_order.
#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    pub hwnd: i64,
    /// (l, t, r, b)
    pub rect: (f64, f64, f64, f64),
    pub title: String,
}

/// One monitor's bounds, desktop px. `work_bottom` is the work-area
/// bottom (excludes the taskbar) when known; falls back to `bottom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub index: i64,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub work_bottom: Option<f64>,
}

impl Monitor {
    /// The work-area bottom (excludes the taskbar) when known; falls back
    /// to `bottom`.
    pub fn floor_y(&self) -> f64 {
        self.work_bottom.unwrap_or(self.bottom)
    }
}

/// One stand-on-able ledge, desktop px. `id` is stable across ticks
/// for the same window/monitor/seam so main.gd (a later slice) can tell
/// "she's still on the same platform" from "the platform moved".
#[derive(Debug, Clone, PartialEq)]
pub struct Platform {
    pub id: String,
    pub hwnd: Option<i64>,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    /// "floor" | "window" | "seam"
    pub kind: String,
}

/// Remove `[c, d)` from every segment in `segments`.
fn subtract_interval(segments: &[(f64, f64)], c: f64, d: f64) -> Vec<(f64, f64)> {
    let mut result = Vec::new();
    for &(a, b) in segments {
        if d <= a || c >= b {
            result.push((a, b));
            continue;
        }
        if c > a {
            result.push((a, c));
        }
        if d < b {
            result.push((d, b));
        }
    }
    result
}

/// Whether `rect` covers `monitor` edge-to-edge within
/// [`FULLSCREEN_TOLERANCE_PX`] on every edge (maximised/fullscreen). The
/// bottom edge is checked against the work-area [`Monitor::floor_y`], not
/// `bottom`.
pub fn covers_whole_monitor(rect: (f64, f64, f64, f64), monitor: &Monitor) -> bool {
    covers_whole_monitor_with_tol(rect, monitor, FULLSCREEN_TOLERANCE_PX)
}

/// [`covers_whole_monitor`] with an explicit tolerance instead of
/// [`FULLSCREEN_TOLERANCE_PX`] (the Python default argument).
pub fn covers_whole_monitor_with_tol(
    rect: (f64, f64, f64, f64),
    monitor: &Monitor,
    tol: f64,
) -> bool {
    let (left, top, right, bottom) = rect;
    left <= monitor.left + tol
        && right >= monitor.right - tol
        && top <= monitor.top + tol
        && bottom >= monitor.floor_y() - tol
}

/// The first monitor in `monitors` whose bounds contain `(cx, cy)`
/// (left/top inclusive, right/bottom exclusive), or `None`.
pub fn monitor_for_point(cx: f64, cy: f64, monitors: &[Monitor]) -> Option<&Monitor> {
    monitors
        .iter()
        .find(|m| m.left <= cx && cx < m.right && m.top <= cy && cy < m.bottom)
}

/// The parts of `[x0, x1]` at height `y` on `monitor` that are (a) within
/// the monitor, (b) not covered by any `blockers` rect whose vertical span
/// includes `y`, (c) at least `min_width` wide, and (d) far enough below
/// the monitor top to sit (SIT_CLEARANCE_FACTOR * her_height_px) - overlap
/// with other windows doesn't matter, only the monitor top does.
fn visible_segments(
    x0: f64,
    x1: f64,
    y: f64,
    monitor: &Monitor,
    blockers: &[Win],
    her_height_px: f64,
    min_width: f64,
) -> Vec<(f64, f64)> {
    if y - monitor.top < SIT_CLEARANCE_FACTOR * her_height_px {
        return Vec::new();
    }
    let lo = x0.max(monitor.left);
    let hi = x1.min(monitor.right);
    if hi - lo <= 0.0 {
        return Vec::new();
    }
    let mut segments = vec![(lo, hi)];
    for blocker in blockers {
        let (bl, bt, br, bb) = blocker.rect;
        // "y strictly inside or on [top,bottom)" of the blocker
        if bt <= y && y < bb {
            segments = subtract_interval(&segments, bl, br);
            if segments.is_empty() {
                return Vec::new();
            }
        }
    }
    segments
        .into_iter()
        .filter(|&(a, b)| b - a >= min_width)
        .collect()
}

/// `windows` must be in TOP-TO-BOTTOM z-order (index 0 = topmost, same
/// as EnumWindows). Returns floors + window top edges + `extra_seams`,
/// each clipped/subtracted/clearance-checked per this module's docstring.
pub fn compute_platforms(
    windows: &[Win],
    monitors: &[Monitor],
    her_height_px: f64,
    extra_seams: &[(f64, f64, f64)],
) -> Vec<Platform> {
    let mut platforms = Vec::new();

    for (i, win) in windows.iter().enumerate() {
        let (left, top, right, bottom) = win.rect;
        let cx = (left + right) / 2.0;
        let cy = (top + bottom) / 2.0;
        let monitor = match monitor_for_point(cx, cy, monitors) {
            Some(monitor) => monitor,
            None => continue,
        };
        if covers_whole_monitor(win.rect, monitor) {
            continue; // maximised/fullscreen - its top IS the monitor top
        }
        let segments = visible_segments(
            left,
            right,
            top,
            monitor,
            &windows[..i],
            her_height_px,
            MIN_SEGMENT_WIDTH_PX,
        );
        for (n, &(sx0, sx1)) in segments.iter().enumerate() {
            platforms.push(Platform {
                id: format!("win:{}:{n}", win.hwnd),
                hwnd: Some(win.hwnd),
                x0: sx0,
                x1: sx1,
                y: top,
                kind: "window".to_string(),
            });
        }
    }

    for monitor in monitors {
        let floor_y = monitor.floor_y();
        let segments = visible_segments(
            monitor.left,
            monitor.right,
            floor_y,
            monitor,
            &[],
            her_height_px,
            MIN_SEGMENT_WIDTH_PX,
        );
        for (n, &(sx0, sx1)) in segments.iter().enumerate() {
            let plat_id = if segments.len() == 1 {
                format!("floor:{}", monitor.index)
            } else {
                format!("floor:{}:{n}", monitor.index)
            };
            platforms.push(Platform {
                id: plat_id,
                hwnd: None,
                x0: sx0,
                x1: sx1,
                y: floor_y,
                kind: "floor".to_string(),
            });
        }
    }

    for (n, &(seam_x0, seam_x1, seam_y)) in extra_seams.iter().enumerate() {
        let monitor = match monitor_for_point((seam_x0 + seam_x1) / 2.0, seam_y, monitors) {
            Some(monitor) => monitor,
            None => continue,
        };
        // A seam is a fixed background feature, not part of the window
        // stack - ANY window overlapping its row occludes it, regardless
        // of z-order among themselves.
        let segments = visible_segments(
            seam_x0,
            seam_x1,
            seam_y,
            monitor,
            windows,
            her_height_px,
            MIN_SEGMENT_WIDTH_PX,
        );
        for (k, &(sx0, sx1)) in segments.iter().enumerate() {
            let plat_id = if segments.len() == 1 {
                format!("seam:{n}")
            } else {
                format!("seam:{n}:{k}")
            };
            platforms.push(Platform {
                id: plat_id,
                hwnd: None,
                x0: sx0,
                x1: sx1,
                y: seam_y,
                kind: "seam".to_string(),
            });
        }
    }

    platforms
}

/// WP-23 slice 3's "never floating" glue: the platform loops.py should
/// send `{"support": ...}` toward when her own platform vanished or no
/// longer contains her x. Candidates are every platform whose x-range
/// contains `x_px`; among those at or below `min_y` (bigger y = lower on
/// screen - she can only ever fall onto something at or below where she
/// last stood, never up), picks the highest one (smallest y) - "the
/// highest platform below her feet containing her x" (the WP's own
/// wording). Falls back to the floor at that x if nothing at/below
/// `min_y` qualifies (a platform she was on vanished with nothing else
/// beneath it in this snapshot). None only if `x_px` is over nothing at
/// all - shouldn't happen once monitor floors span every monitor, but the
/// caller must handle it (there's nothing to send a `support` toward).
pub fn pick_landing_platform(platforms: &[Platform], x_px: f64, min_y: f64) -> Option<&Platform> {
    let candidates: Vec<&Platform> = platforms
        .iter()
        .filter(|p| p.x0 <= x_px && x_px <= p.x1)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let below: Vec<&Platform> = candidates
        .iter()
        .copied()
        .filter(|p| p.y >= min_y - 0.5)
        .collect();
    if let Some(picked) = below.iter().copied().min_by(|a, b| a.y.total_cmp(&b.y)) {
        return Some(picked);
    }
    let floors: Vec<&Platform> = candidates
        .iter()
        .copied()
        .filter(|p| p.kind == "floor")
        .collect();
    floors.iter().copied().min_by(|a, b| a.y.total_cmp(&b.y))
}
