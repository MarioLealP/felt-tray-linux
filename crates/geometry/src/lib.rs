//! Where the overlay's visible frame actually is, in main.gd's world units.
//!
//! Audit 2026-09-24 (WP-10): presence used a hardcoded exit x of 4.0, but the
//! real visible half-width on the 3440x1440 primary is ~5.5, so "walking off
//! screen" stopped on screen.
//!
//! WP-22: the overlay now has one window per monitor, all rendering the same
//! world, and every camera is framed through ONE map between virtual-desktop
//! pixels and world units (main.gd `_px_to_world`):
//! ```text
//! world_x = (origin_px - desktop_x) / ppm        (desktop-right = world -x)
//! ppm     = primary_window_height / (character_height * VERTICAL_MARGIN)
//! ```
//! The primary window's centre is world x = 0 and the floor (world y = 0) is the
//! same desktop row on every monitor. So the floor she walks on runs from the
//! leftmost monitor's left edge (world x_max) to the rightmost monitor's right
//! edge (world x_min): here +5.48 .. -11.60 (DISPLAY2 is -5.48 .. -11.60).
//!
//! main.gd reports all of it on connect (`{"geometry": {ppm, origin_px, x_min,
//! x_max, roam_min, roam_max, views, pid, ...}}`); that report wins. Until it
//! arrives, [`estimate_from_monitors`] rebuilds the same numbers from the
//! monitor layout (see [`DisplayMetrics`]) and FELT_CHARACTER_HEIGHT /
//! FELT_CHARACTER_WIDTH. Assumes the monitors sit side by side (one continuous
//! floor).
//!
//! DPI caveat: both monitors are at 100 % scaling. main.gd uses physical
//! pixels and one ppm for every monitor; if a monitor ever gets a different
//! scale, ppm (here and in main.gd) has to become per monitor, and this
//! process must be per-monitor-DPI aware to read physical cursor/window
//! coordinates.

use serde_json::{Map, Value};

/// main.gd's VERTICAL_MARGIN - fixed by standing rule (her size never changes).
pub const VERTICAL_MARGIN: f64 = 2.667;
/// How far past the frame edge (beyond her own half-width) an exit target goes.
pub const OFFSCREEN_MARGIN: f64 = 0.5;

/// Win32 `GetSystemMetrics` index for the primary monitor's width.
pub const SM_CXSCREEN: i32 = 0;
/// Win32 `GetSystemMetrics` index for the primary monitor's height.
pub const SM_CYSCREEN: i32 = 1;
/// Win32 `MONITORINFOF_PRIMARY` - the primary-monitor flag in `dwFlags`.
pub const MONITORINFOF_PRIMARY: u32 = 1;

/// One monitor's rect in virtual-desktop pixels: `(x, y, w, h)`.
pub type MonitorRect = (i32, i32, i32, i32);

/// All monitors' rects plus the primary's index:
/// `([(x, y, w, h) per monitor], index of the primary)`.
pub type MonitorRects = Option<(Vec<MonitorRect>, usize)>;

/// The Win32 display calls this module's estimates were written against
/// (`GetSystemMetrics`, `EnumDisplayMonitors`, `GetMonitorInfoW`). No
/// implementation ships with this crate (pure logic only); the platform layer
/// provides one. Failures log and return `None`, like the Python originals.
pub trait DisplayMetrics {
    /// The primary monitor's size. Only the aspect ratio is used, so DPI
    /// virtualization doesn't matter. `None` on failure (including a
    /// non-positive width or height).
    fn primary_screen_size(&self) -> Option<(i32, i32)>;

    /// (`[(x, y, w, h)` per monitor`], index of the primary), in the order
    /// Windows enumerates them, or `None` on failure.
    fn monitor_rects(&self) -> MonitorRects;
}

/// Python's `max(a, b)` for floats: returns `a` unless `b > a` (so a NaN
/// argument propagates, unlike `f64::max`).
fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python's `min(a, b)` for floats: returns `a` unless `b < a`.
fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `float()` on a JSON value: numbers convert, `true`/`false` become
/// `1.0`/`0.0`, numeric strings parse; everything else raises (`None` here).
fn py_float(value: &Value) -> Option<f64> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Python's `int()` on a JSON value: integral numbers pass through, floats
/// truncate toward zero, integer strings parse; everything else raises
/// (`None` here).
fn py_int(value: &Value) -> Option<i64> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// [`py_int`] narrowed to the `i32` range virtual-desktop pixels live in.
fn py_pixel(value: &Value) -> Option<i32> {
    py_int(value).and_then(|i| i32::try_from(i).ok())
}

/// Python's `str()` on a JSON scalar (used for the view name); composites fall
/// back to their JSON form.
fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(b) => {
            if *b {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else {
                let s = n.as_f64().map_or_else(|| n.to_string(), |f| format!("{f}"));
                if s.contains(['.', 'e', 'i', 'N']) {
                    s
                } else {
                    format!("{s}.0")
                }
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A truthiness check mirroring Python's `if report.get("pid")` on JSON data.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// One overlay window (= one monitor), virtual-desktop pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl View {
    pub fn new(name: impl Into<String>, x: i32, y: i32, w: i32, h: i32) -> Self {
        Self {
            name: name.into(),
            x,
            y,
            w,
            h,
        }
    }

    /// Python's `self.x <= px < self.x + self.w`.
    pub fn contains_x(&self, px: f64) -> bool {
        let x = f64::from(self.x);
        x <= px && px < x + f64::from(self.w)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverlayGeometry {
    /// The primary window's own frame (the pre-WP-22 fields, still used as a
    /// fallback when nothing else is known).
    pub visible_half_width: f64,
    pub roam_limit: f64,
    pub character_half_width: f64,
    /// WP-22: the pixel <-> world map and the whole multi-monitor floor.
    pub ppm: f64,
    pub origin_px: f64,
    pub x_min: Option<f64>,
    pub x_max: Option<f64>,
    pub roam_min: Option<f64>,
    pub roam_max: Option<f64>,
    pub views: Vec<View>,
    pub pid: Option<i64>,
}

impl OverlayGeometry {
    /// The pre-WP-22 primary-only frame; the pixel map starts unset.
    pub fn new(visible_half_width: f64, roam_limit: f64, character_half_width: f64) -> Self {
        Self {
            visible_half_width,
            roam_limit,
            character_half_width,
            ..Self::default()
        }
    }

    // -- the floor span ----------------------------------------------------------

    /// `(x_min, x_max)`: the world x the windows cover (x_max = screen left).
    pub fn span(&self) -> (f64, f64) {
        if let (Some(x_min), Some(x_max)) = (self.x_min, self.x_max) {
            (x_min, x_max)
        } else {
            (-self.visible_half_width, self.visible_half_width)
        }
    }

    /// `(lo, hi)`: where her pivot may stand without clipping an outer edge.
    pub fn roam(&self) -> (f64, f64) {
        if let (Some(roam_min), Some(roam_max)) = (self.roam_min, self.roam_max) {
            (roam_min, roam_max)
        } else {
            (-self.roam_limit, self.roam_limit)
        }
    }

    pub fn clamp_roam(&self, x: f64) -> f64 {
        let (lo, hi) = self.roam();
        py_max(lo, py_min(hi, x))
    }

    /// `|x|` at which she is fully outside the PRIMARY's frame (legacy).
    pub fn offscreen_x(&self) -> f64 {
        self.visible_half_width + self.character_half_width + OFFSCREEN_MARGIN
    }

    /// World x fully past the desktop's LEFT edge (world +x side).
    pub fn offscreen_left_x(&self) -> f64 {
        self.span().1 + self.character_half_width + OFFSCREEN_MARGIN
    }

    /// World x fully past the desktop's RIGHT edge (world -x side).
    pub fn offscreen_right_x(&self) -> f64 {
        self.span().0 - self.character_half_width - OFFSCREEN_MARGIN
    }

    // -- pixels <-> world ------------------------------------------------------------

    pub fn has_pixel_map(&self) -> bool {
        self.ppm > 0.0
    }

    /// A virtual-desktop x pixel -> world x (unclamped). Without a pixel
    /// map (an old primary-only overlay), the primary is assumed to span
    /// 0..(2 * visible_half_width) world units' worth of pixels - callers
    /// should prefer [`has_pixel_map`](Self::has_pixel_map).
    pub fn px_to_world_x(&self, desktop_x: f64) -> f64 {
        if self.has_pixel_map() {
            (self.origin_px - desktop_x) / self.ppm
        } else {
            0.0
        }
    }

    pub fn world_to_px_x(&self, world_x: f64) -> f64 {
        self.origin_px - world_x * self.ppm
    }

    /// Come here: the world x under that desktop pixel, on any monitor,
    /// clamped to where she can stand without clipping an outer edge.
    pub fn cursor_px_to_world_x(&self, desktop_x: f64) -> f64 {
        self.clamp_roam(self.px_to_world_x(desktop_x))
    }

    /// 0..1 across the primary monitor -> world x under that point,
    /// clamped to where she can stand (pre-WP-22 mapping, kept for a
    /// primary-only fallback).
    pub fn cursor_fraction_to_world_x(&self, fraction: f64) -> f64 {
        // main.gd's camera looks along +z, so its right is world -x: screen-RIGHT is
        // world -x. The old mapping assumed the opposite, so "come here" walked away
        // from the cursor and "shoo" walked toward it (found in WP-21, 2026-09-26).
        let x = (0.5 - fraction) * 2.0 * self.visible_half_width;
        self.clamp_roam(x)
    }

    pub fn view_at_world_x(&self, world_x: f64) -> Option<&View> {
        if self.views.is_empty() || !self.has_pixel_map() {
            return None;
        }
        let px = self.world_to_px_x(world_x);
        if let Some(v) = self.views.iter().find(|v| v.contains_x(px)) {
            return Some(v);
        }
        let edge_dist = |v: &View| {
            py_min(
                (px - f64::from(v.x)).abs(),
                (px - (f64::from(v.x) + f64::from(v.w))).abs(),
            )
        };
        self.views
            .iter()
            .min_by(|a, b| edge_dist(a).total_cmp(&edge_dist(b)))
    }

    /// `(lo, hi)` world x of one monitor's floor, clamped to the roam range.
    pub fn view_span(&self, view: &View) -> (f64, f64) {
        let lo = self.px_to_world_x(f64::from(view.x) + f64::from(view.w));
        let hi = self.px_to_world_x(f64::from(view.x));
        let (rlo, rhi) = self.roam();
        (py_max(lo, rlo), py_min(hi, rhi))
    }

    /// A desktop window's left/right pixels -> (centre world x, half width in m).
    pub fn window_rect_to_world(&self, left: f64, right: f64) -> (f64, f64) {
        let a = self.px_to_world_x(left);
        let b = self.px_to_world_x(right);
        ((a + b) / 2.0, (a - b).abs() / 2.0)
    }
}

/// Pre-WP-22 estimate: the primary monitor alone.
pub fn estimate_geometry(
    screen_width: f64,
    screen_height: f64,
    character_height: f64,
    character_width: f64,
) -> OverlayGeometry {
    estimate_geometry_with_margin(
        screen_width,
        screen_height,
        character_height,
        character_width,
        VERTICAL_MARGIN,
    )
}

/// [`estimate_geometry`] with an explicit `vertical_margin`
/// (Python's default argument).
pub fn estimate_geometry_with_margin(
    screen_width: f64,
    screen_height: f64,
    character_height: f64,
    character_width: f64,
    vertical_margin: f64,
) -> OverlayGeometry {
    let aspect = if screen_height > 0.0 {
        screen_width / screen_height
    } else {
        16.0 / 9.0
    };
    let visible_half_width = character_height * vertical_margin * 0.5 * aspect;
    OverlayGeometry {
        visible_half_width,
        roam_limit: py_max(visible_half_width - character_width, 0.0),
        character_half_width: character_width / 2.0,
        ..OverlayGeometry::default()
    }
}

/// main.gd's own layout rebuilt from the monitor rects (x, y, w, h): each
/// window is its monitor minus the top pixel (the fullscreen-optimisation
/// dodge), ppm from the primary window's height, world x = 0 at the
/// primary window's centre.
pub fn estimate_from_monitors(
    monitors: &[(i32, i32, i32, i32)],
    primary_index: usize,
    character_height: f64,
    character_width: f64,
) -> OverlayGeometry {
    estimate_from_monitors_with_margin(
        monitors,
        primary_index,
        character_height,
        character_width,
        VERTICAL_MARGIN,
    )
}

/// [`estimate_from_monitors`] with an explicit `vertical_margin`
/// (Python's default argument).
pub fn estimate_from_monitors_with_margin(
    monitors: &[(i32, i32, i32, i32)],
    primary_index: usize,
    character_height: f64,
    character_width: f64,
    vertical_margin: f64,
) -> OverlayGeometry {
    let mut views: Vec<View> = monitors
        .iter()
        .enumerate()
        .map(|(i, &(x, y, w, h))| {
            let name = if i == primary_index {
                "primary".to_owned()
            } else {
                format!("screen{i}")
            };
            View::new(name, x, y + 1, w, h - 1)
        })
        .collect();
    let main = views[primary_index].clone();
    let cam_size = character_height * vertical_margin;
    let ppm = f64::from(main.h) / cam_size;
    let origin = f64::from(main.x) + f64::from(main.w) / 2.0;
    let x_min = views
        .iter()
        .map(|v| (origin - (f64::from(v.x) + f64::from(v.w))) / ppm)
        .reduce(py_min)
        .expect("at least one monitor");
    let x_max = views
        .iter()
        .map(|v| (origin - f64::from(v.x)) / ppm)
        .reduce(py_max)
        .expect("at least one monitor");
    let half = cam_size * 0.5 * f64::from(main.w) / f64::from(main.h);
    views.sort_by_key(|v| v.x);
    OverlayGeometry {
        visible_half_width: half,
        roam_limit: py_max(half - character_width, 0.0),
        character_half_width: character_width / 2.0,
        ppm,
        origin_px: origin,
        x_min: Some(x_min),
        x_max: Some(x_max),
        roam_min: Some(py_min(x_min + character_width, 0.0)),
        roam_max: Some(py_max(x_max - character_width, 0.0)),
        views,
        pid: None,
    }
}

/// `float(report.get(key, default))` - `None` when the key exists but its
/// value doesn't convert.
fn report_float(obj: &Map<String, Value>, key: &str, default: f64) -> Option<f64> {
    match obj.get(key) {
        Some(v) => py_float(v),
        None => Some(default),
    }
}

/// The WP-22 pixel-map half of [`from_overlay_report`]. Every Python failure
/// path there (missing key, bad type, empty views, inverted span) maps to
/// `None`, which keeps just the legacy fields.
fn pixel_map_from_report(
    obj: &Map<String, Value>,
    half: f64,
    roam: f64,
    char_half: f64,
) -> Option<OverlayGeometry> {
    let ppm = py_float(obj.get("ppm")?)?;
    let origin = obj.get("origin_px")?;
    let origin_x = match origin {
        Value::Array(a) => py_float(a.first()?)?,
        other => py_float(other)?,
    };
    let mut views: Vec<View> = Vec::new();
    for v in obj.get("views")?.as_array()? {
        let v = v.as_object()?;
        let name = match v.get("name") {
            Some(n) => py_str(n),
            None => String::new(),
        };
        views.push(View::new(
            name,
            py_pixel(v.get("x")?)?,
            py_pixel(v.get("y")?)?,
            py_pixel(v.get("w")?)?,
            py_pixel(v.get("h")?)?,
        ));
    }
    views.sort_by_key(|v| v.x);
    let x_min = py_float(obj.get("x_min")?)?;
    let x_max = py_float(obj.get("x_max")?)?;
    let roam_min = match obj.get("roam_min") {
        Some(v) => py_float(v)?,
        None => py_min(x_min + 2.0 * char_half, 0.0),
    };
    let roam_max = match obj.get("roam_max") {
        Some(v) => py_float(v)?,
        None => py_max(x_max - 2.0 * char_half, 0.0),
    };
    let pid = match obj.get("pid") {
        Some(v) if is_truthy(v) => Some(py_int(v)?),
        _ => None,
    };
    if ppm <= 0.0 || views.is_empty() || x_min >= x_max {
        return None;
    }
    Some(OverlayGeometry {
        visible_half_width: half,
        roam_limit: py_max(roam, 0.0),
        character_half_width: char_half,
        ppm,
        origin_px: origin_x,
        x_min: Some(x_min),
        x_max: Some(x_max),
        roam_min: Some(roam_min),
        roam_max: Some(roam_max),
        views,
        pid,
    })
}

/// main.gd's own numbers, when it sends them; anything missing or
/// malformed falls back to the estimate.
pub fn from_overlay_report(report: &Value, fallback: OverlayGeometry) -> OverlayGeometry {
    let Some(obj) = report.as_object() else {
        // Python's AttributeError: a non-dict report has no .get.
        return fallback;
    };
    // -- the legacy fields: any bad value rejects the whole report ----------
    let Some(half) = report_float(obj, "visible_half_width", fallback.visible_half_width) else {
        return fallback;
    };
    let Some(roam) = report_float(obj, "roam_x_limit", fallback.roam_limit) else {
        return fallback;
    };
    let Some(char_width) =
        report_float(obj, "character_width", fallback.character_half_width * 2.0)
    else {
        return fallback;
    };
    let char_half = char_width / 2.0;
    if half <= 0.0 {
        return fallback;
    }
    let base = OverlayGeometry::new(half, py_max(roam, 0.0), char_half);
    // -- the WP-22 pixel map: any bad value keeps just the legacy fields ----
    pixel_map_from_report(obj, half, roam, char_half).unwrap_or(base)
}
