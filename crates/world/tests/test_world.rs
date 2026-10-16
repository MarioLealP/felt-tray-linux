//! WP-23 slice 1: world.py's pure platform computation, on synthetic
//! window/monitor layouts (no Win32 - windows_win32.py is the untested Win32
//! adapter, same boundary as felt-tray's capture_win32.py).

use std::collections::{HashMap, HashSet};

use felt_world::{Monitor, Platform, Win, compute_platforms, pick_landing_platform};

const HER_HEIGHT_PX: f64 = 540.0; // 0.6 * 540 = 324 px sit clearance

const MONITOR0: Monitor = Monitor {
    index: 0,
    left: 0.0,
    top: 0.0,
    right: 1920.0,
    bottom: 1080.0,
    work_bottom: Some(1040.0),
};

fn ids(platforms: &[Platform]) -> Vec<&str> {
    platforms.iter().map(|p| p.id.as_str()).collect()
}

#[test]
fn test_single_window_contributes_a_platform_plus_the_floor() {
    let win = Win {
        hwnd: 111,
        rect: (100.0, 400.0, 900.0, 800.0),
        title: "Notepad".to_string(),
    };
    let platforms = compute_platforms(&[win], &[MONITOR0], HER_HEIGHT_PX, &[]);

    let by_id: HashMap<&str, &Platform> = platforms.iter().map(|p| (p.id.as_str(), p)).collect();
    assert_eq!(
        by_id.keys().copied().collect::<HashSet<_>>(),
        HashSet::from(["win:111:0", "floor:0"])
    );
    let window_plat = by_id["win:111:0"];
    assert_eq!(window_plat.kind, "window");
    assert_eq!(window_plat.hwnd, Some(111));
    assert_eq!(
        (window_plat.x0, window_plat.x1, window_plat.y),
        (100.0, 900.0, 400.0)
    );
    let floor_plat = by_id["floor:0"];
    assert_eq!(floor_plat.kind, "floor");
    assert!(floor_plat.hwnd.is_none());
    assert_eq!(
        (floor_plat.x0, floor_plat.x1, floor_plat.y),
        (0.0, 1920.0, 1040.0)
    );
}

#[test]
fn test_window_partly_covered_by_a_higher_window_splits_into_two_segments() {
    let higher = Win {
        hwnd: 222,
        rect: (400.0, 300.0, 600.0, 500.0),
        title: "Popup".to_string(),
    }; // covers y=400, x 400-600
    let lower = Win {
        hwnd: 111,
        rect: (100.0, 400.0, 900.0, 800.0),
        title: "Notepad".to_string(),
    };
    // higher is FIRST -> topmost in z-order.
    let platforms = compute_platforms(&[higher, lower], &[MONITOR0], HER_HEIGHT_PX, &[]);

    let lower_segments: HashMap<&str, &Platform> = platforms
        .iter()
        .filter(|p| p.hwnd == Some(111))
        .map(|p| (p.id.as_str(), p))
        .collect();
    assert_eq!(
        lower_segments.keys().copied().collect::<HashSet<_>>(),
        HashSet::from(["win:111:0", "win:111:1"])
    );
    let seg0 = lower_segments["win:111:0"];
    let seg1 = lower_segments["win:111:1"];
    assert_eq!((seg0.x0, seg0.x1), (100.0, 400.0));
    assert_eq!((seg1.x0, seg1.x1), (600.0, 900.0));
    // higher's own top edge (y=300) is too close to the monitor top (< 324) - dropped.
    assert!(!platforms.iter().any(|p| p.hwnd == Some(222)));
}

#[test]
fn test_window_fully_covered_by_a_higher_window_contributes_nothing() {
    let higher = Win {
        hwnd: 222,
        rect: (50.0, 350.0, 950.0, 900.0),
        title: "Fullscreen-ish".to_string(),
    };
    let lower = Win {
        hwnd: 111,
        rect: (100.0, 400.0, 900.0, 800.0),
        title: "Notepad".to_string(),
    };
    let platforms = compute_platforms(&[higher, lower], &[MONITOR0], HER_HEIGHT_PX, &[]);

    assert!(!platforms.iter().any(|p| p.hwnd == Some(111)));
}

#[test]
fn test_window_too_close_to_the_monitor_top_is_dropped() {
    let win = Win {
        hwnd: 111,
        rect: (100.0, 100.0, 900.0, 300.0),
        title: "Notepad".to_string(),
    }; // y=100 < 324
    let platforms = compute_platforms(&[win], &[MONITOR0], HER_HEIGHT_PX, &[]);

    assert!(!platforms.iter().any(|p| p.hwnd == Some(111)));
}

#[test]
fn test_maximised_window_contributes_no_top_edge_platform() {
    let maximised = Win {
        hwnd: 111,
        rect: (0.0, 0.0, 1920.0, 1040.0),
        title: "Maximised".to_string(),
    }; // exactly the work area
    let fullscreen = Win {
        hwnd: 222,
        rect: (0.0, 0.0, 1920.0, 1080.0),
        title: "Fullscreen".to_string(),
    }; // exactly the monitor
    let platforms = compute_platforms(&[maximised, fullscreen], &[MONITOR0], HER_HEIGHT_PX, &[]);

    assert!(
        !platforms
            .iter()
            .any(|p| p.hwnd == Some(111) || p.hwnd == Some(222))
    );
    assert_eq!(ids(&platforms), vec!["floor:0"]);
}

#[test]
fn test_two_monitors_with_different_floor_heights() {
    let monitor1 = Monitor {
        index: 1,
        left: 1920.0,
        top: 200.0,
        right: 2880.0,
        bottom: 1400.0,
        work_bottom: Some(1400.0),
    };
    let platforms = compute_platforms(&[], &[MONITOR0, monitor1], HER_HEIGHT_PX, &[]);

    let by_id: HashMap<&str, &Platform> = platforms.iter().map(|p| (p.id.as_str(), p)).collect();
    assert_eq!(
        by_id.keys().copied().collect::<HashSet<_>>(),
        HashSet::from(["floor:0", "floor:1"])
    );
    assert_eq!(by_id["floor:0"].y, 1040.0);
    assert_eq!(by_id["floor:1"].y, 1400.0);
    assert_eq!((by_id["floor:1"].x0, by_id["floor:1"].x1), (1920.0, 2880.0));
}

#[test]
fn test_seam_at_y_897_on_a_side_monitor_partly_covered_by_a_window() {
    // Mario's DISPLAY2 2x2 grid: the side monitor's bottom quadrants' top
    // edge sits 540 px below that screen's top (here top=357 -> y=897).
    let side_monitor = Monitor {
        index: 1,
        left: 1920.0,
        top: 357.0,
        right: 2880.0,
        bottom: 1437.0,
        work_bottom: Some(1437.0),
    };
    let covering_window = Win {
        hwnd: 333,
        rect: (1920.0, 700.0, 2400.0, 1000.0),
        title: "Explorer".to_string(),
    };
    let platforms = compute_platforms(
        &[covering_window],
        &[MONITOR0, side_monitor],
        HER_HEIGHT_PX,
        &[(1920.0, 2880.0, 897.0)],
    );

    let seam_platforms: Vec<&Platform> = platforms.iter().filter(|p| p.kind == "seam").collect();
    assert_eq!(seam_platforms.len(), 1);
    let seam = seam_platforms[0];
    assert_eq!(seam.id, "seam:0");
    assert_eq!((seam.x0, seam.x1, seam.y), (2400.0, 2880.0, 897.0));
}

#[test]
fn test_segment_narrower_than_min_width_is_dropped() {
    let higher = Win {
        hwnd: 222,
        rect: (100.0, 300.0, 850.0, 500.0),
        title: "Almost full cover".to_string(),
    };
    let lower = Win {
        hwnd: 111,
        rect: (100.0, 400.0, 900.0, 800.0),
        title: "Notepad".to_string(),
    };
    // Remaining sliver is 900-850=50px, under MIN_SEGMENT_WIDTH_PX (120).
    let platforms = compute_platforms(&[higher, lower], &[MONITOR0], HER_HEIGHT_PX, &[]);

    assert!(!platforms.iter().any(|p| p.hwnd == Some(111)));
}

// -- pick_landing_platform (WP-23 slice 3's "never floating" glue) -----------

fn floor() -> Platform {
    Platform {
        id: "floor:0".to_string(),
        hwnd: None,
        x0: 0.0,
        x1: 1920.0,
        y: 1040.0,
        kind: "floor".to_string(),
    }
}

fn window_ledge() -> Platform {
    Platform {
        id: "win:1:0".to_string(),
        hwnd: Some(1),
        x0: 100.0,
        x1: 900.0,
        y: 600.0,
        kind: "window".to_string(),
    }
}

fn lower_ledge() -> Platform {
    Platform {
        id: "win:2:0".to_string(),
        hwnd: Some(2),
        x0: 100.0,
        x1: 900.0,
        y: 800.0,
        kind: "window".to_string(),
    }
}

#[test]
fn test_pick_landing_platform_prefers_the_highest_platform_below_her_feet() {
    // She was standing at y=600 (window_ledge); it vanished. lower_ledge
    // (y=800) and floor (y=1040) both qualify (below her old feet) - the
    // highest of the two (lower_ledge) wins.
    let platforms = [floor(), lower_ledge()];
    let picked = pick_landing_platform(&platforms, 500.0, 600.0);
    assert_eq!(picked.map(|p| p.id.as_str()), Some("win:2:0"));
}

#[test]
fn test_pick_landing_platform_falls_back_to_the_floor_when_nothing_qualifies() {
    // Only a platform ABOVE her last height is left at this x - not a valid
    // landing (she can't fall upward) - the floor is the fallback.
    let platforms = [floor(), window_ledge()];
    let picked = pick_landing_platform(&platforms, 500.0, 900.0);
    assert_eq!(picked.map(|p| p.id.as_str()), Some("floor:0"));
}

#[test]
fn test_pick_landing_platform_none_when_x_is_over_nothing() {
    let platforms = [window_ledge()];
    let picked = pick_landing_platform(&platforms, 5000.0, 0.0);
    assert!(picked.is_none());
}

#[test]
fn test_pick_landing_platform_ignores_candidates_not_containing_x() {
    // lower_ledge's x-range doesn't cover x=1500 - only the floor qualifies.
    let other_floor = Platform {
        id: "floor:1".to_string(),
        hwnd: None,
        x0: 1000.0,
        x1: 2000.0,
        y: 1040.0,
        kind: "floor".to_string(),
    };
    let platforms = [lower_ledge(), other_floor];
    let picked = pick_landing_platform(&platforms, 1500.0, 0.0);
    assert_eq!(picked.map(|p| p.id.as_str()), Some("floor:1"));
}
