//! geometry.py (audit 2026-09-24, WP-10): the visible frame derived from
//! main.gd's camera framing, instead of a hardcoded 4.0 exit x.

use felt_geometry::{
    MonitorRect, OverlayGeometry, View, estimate_from_monitors, estimate_geometry,
    from_overlay_report,
};
use serde_json::{Value, json};

// Mario's layout: DISPLAY1 3440x1440 at (0,0), DISPLAY2 1920x1080 at (3440,357).
const MONITORS: &[(i32, i32, i32, i32)] = &[(0, 0, 3440, 1440), (3440, 357, 1920, 1080)];

/// main.gd's own report from the live overlay (2026-09-26, WP-22 spike).
fn report() -> Value {
    json!({
        "center_y": 1.94914617221805,
        "character_height": 1.71961998939514,
        "character_width": 1.32344222068787,
        "floor_px_y": 1332.07495117188,
        "origin_px": [1720.0, 720.5],
        "pid": 43156,
        "ppm": 313.76557863193,
        "roam_max": 4.15835738182068,
        "roam_min": -10.2775757312775,
        "roam_x_limit": 4.15835730461216,
        "views": [
            {"h": 1439, "name": "primary", "w": 3440, "x": 0, "y": 1},
            {"h": 1079, "name": "screen1", "w": 1920, "x": 3440, "y": 358},
        ],
        "visible_half_width": 5.48179952530003,
        "x_max": 5.48179960250854,
        "x_min": -11.6010179519653,
    })
}

/// REPORT is a real relay-report literal with heterogeneous value types
/// (floats, a pixel-pair list, a list of view dicts) - narrows the one
/// numeric field these tests divide by instead of subscripting the whole
/// union inline.
fn _report_ppm() -> f64 {
    let report = report();
    let value = &report["ppm"];
    assert!(value.is_f64());
    value.as_f64().expect("asserted is_f64")
}

#[test]
fn test_estimate_matches_main_gd_framing_on_the_ultrawide_primary() {
    let geo = estimate_geometry(3440.0, 1440.0, 1.7196, 1.3234);
    // cam.size = 1.7196 * 2.667 = 4.586; half width = 4.586 / 2 * 3440/1440
    assert!((geo.visible_half_width - 5.478).abs() < 0.01);
    assert!((geo.roam_limit - (geo.visible_half_width - 1.3234)).abs() < 1e-9);
    // the old hardcoded exit (4.0) was inside the frame; the new one isn't
    assert!(geo.offscreen_x() > geo.visible_half_width + 0.6);
    assert!(geo.visible_half_width > 4.0);
}

#[test]
fn test_cursor_mapping_covers_the_frame_and_clamps_to_roam() {
    let geo = OverlayGeometry::new(5.0, 4.0, 0.5);
    // screen-right is world -x (camera looks along +z)
    assert_eq!(geo.cursor_fraction_to_world_x(0.5), 0.0);
    assert!((geo.cursor_fraction_to_world_x(0.7) + 2.0).abs() < 1e-9);
    assert_eq!(geo.cursor_fraction_to_world_x(1.0), -4.0);
    assert_eq!(geo.cursor_fraction_to_world_x(0.0), 4.0);
}

#[test]
fn test_overlay_report_wins_and_bad_reports_fall_back() {
    let fallback = OverlayGeometry::new(5.0, 4.0, 0.5);
    let reported = from_overlay_report(
        &json!({"visible_half_width": 6.0, "roam_x_limit": 4.5}),
        fallback.clone(),
    );
    assert_eq!(reported.visible_half_width, 6.0);
    assert_eq!(reported.roam_limit, 4.5);
    assert_eq!(
        from_overlay_report(&json!({"visible_half_width": "nope"}), fallback.clone()),
        fallback
    );
    assert_eq!(
        from_overlay_report(&json!({"visible_half_width": 0}), fallback.clone()),
        fallback
    );
}

// -- WP-22: both monitors -------------------------------------------------------

#[test]
fn test_monitor_estimate_matches_what_main_gd_reports() {
    let est = estimate_from_monitors(MONITORS, 0, 1.71962, 1.32344);
    let rep = from_overlay_report(&report(), est.clone());
    assert!((est.ppm - rep.ppm).abs() < 1e-3);
    assert_eq!(est.origin_px, 1720.0);
    assert_eq!(rep.origin_px, 1720.0);
    assert!((est.span().0 - rep.span().0).abs() < 1e-3);
    assert!((est.span().1 - rep.span().1).abs() < 1e-3);
    assert!((est.roam().0 - rep.roam().0).abs() < 1e-3);
    assert!((est.roam().1 - rep.roam().1).abs() < 1e-3);
    let dims = |g: &OverlayGeometry| -> Vec<MonitorRect> {
        g.views.iter().map(|v| (v.x, v.y, v.w, v.h)).collect()
    };
    assert_eq!(dims(&est), dims(&rep));
    assert_eq!(rep.pid, Some(43156));
}

#[test]
fn test_pixel_map_is_continuous_across_the_monitor_seam() {
    let geo = from_overlay_report(&report(), estimate_geometry(3440.0, 1440.0, 1.7196, 1.3234));
    // desktop-right is world -x; the seam (x=3440) is the primary's right edge
    assert_eq!(geo.px_to_world_x(1720.0), 0.0);
    assert!((geo.px_to_world_x(3440.0) + geo.visible_half_width).abs() < 1e-3);
    assert!(geo.px_to_world_x(3441.0) < geo.px_to_world_x(3439.0));
    assert!((geo.world_to_px_x(geo.px_to_world_x(4400.0)) - 4400.0).abs() < 1e-9);
    // the floor runs across both monitors
    assert!(geo.span().0 < -11.5 && geo.span().1 > 5.4);
    let primary = geo.view_at_world_x(0.0);
    let screen1 = geo.view_at_world_x(-8.5);
    assert!(primary.is_some_and(|v| v.name == "primary"));
    assert!(screen1.is_some_and(|v| v.name == "screen1"));
    let (lo, hi) = geo.view_span(&geo.views[1]);
    assert!((hi + geo.visible_half_width).abs() < 1e-3 && lo == geo.roam().0);
}

#[test]
fn test_come_here_cursor_mapping_spans_both_monitors_and_clamps() {
    let geo = from_overlay_report(&report(), estimate_geometry(3440.0, 1440.0, 1.7196, 1.3234));
    assert!((geo.cursor_px_to_world_x(4400.0) - (1720.0 - 4400.0) / _report_ppm()).abs() < 1e-9);
    assert_eq!(geo.cursor_px_to_world_x(5359.0), geo.roam().0); // DISPLAY2's right edge
    assert_eq!(geo.cursor_px_to_world_x(0.0), geo.roam().1); // the primary's left edge
    // offscreen targets sit past each outer edge of the whole desktop
    assert!(geo.offscreen_left_x() > geo.span().1 + geo.character_half_width);
    assert!(geo.offscreen_right_x() < geo.span().0 - geo.character_half_width);
}

#[test]
fn test_focused_window_rect_maps_to_world() {
    let geo = from_overlay_report(&report(), estimate_geometry(3440.0, 1440.0, 1.7196, 1.3234));
    let (centre, half) = geo.window_rect_to_world(3440.0, 4400.0); // DISPLAY2's left half
    assert!((centre - geo.px_to_world_x(3920.0)).abs() < 1e-9);
    assert!((half - 480.0 / _report_ppm()).abs() < 1e-9);
}

#[test]
fn test_old_overlay_report_without_the_pixel_map_keeps_the_symmetric_frame() {
    let fallback = estimate_geometry(3440.0, 1440.0, 1.7196, 1.3234);
    let old = from_overlay_report(
        &json!({"visible_half_width": 5.5, "roam_x_limit": 4.2}),
        fallback.clone(),
    );
    assert!(!old.has_pixel_map() && old.views.is_empty());
    assert_eq!(old.span(), (-5.5, 5.5));
    assert_eq!(old.roam(), (-4.2, 4.2));
    assert!(old.view_at_world_x(0.0).is_none());
    // a broken pixel map falls back to the legacy fields too
    let mut broken = report();
    broken["views"] = json!([]);
    assert!(!from_overlay_report(&broken, fallback).has_pixel_map());
}

#[test]
fn test_single_monitor_estimate_has_no_side_view() {
    let geo = estimate_from_monitors(&[(0, 0, 3440, 1440)], 0, 1.7196, 1.3234);
    let names: Vec<&str> = geo.views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["primary"]);
    // symmetric around the primary's centre
    assert!((geo.span().0 + geo.span().1).abs() < 1e-9);
    // (Python: isinstance(geo.views[0], View) - true by construction here)
    let _: &View = &geo.views[0];
}
