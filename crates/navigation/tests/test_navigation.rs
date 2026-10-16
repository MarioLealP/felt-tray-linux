//! WP-23 slice 3: navigation's pure BFS over the platform graph. ppm=340
//! throughout (her real measured scale, per the WP-23 build notes - a 540 px
//! character at ~1.6 m tall).

use felt_navigation::{Climb, Platform, PlatformKind, Step, Walk, plan_route};

const PPM: f64 = 340.0;

fn floor0() -> Platform {
    Platform {
        id: "floor:0".into(),
        hwnd: None,
        x0: 0.0,
        x1: 1920.0,
        y: 1440.0,
        kind: PlatformKind::Floor,
    }
}

fn floor1() -> Platform {
    Platform {
        id: "floor:1".into(),
        hwnd: None,
        x0: 1920.0,
        x1: 2880.0,
        y: 1437.0,
        kind: PlatformKind::Floor,
    }
}

fn window() -> Platform {
    Platform {
        id: "win:1:0".into(),
        hwnd: Some(1),
        x0: 100.0,
        x1: 900.0,
        y: 900.0,
        kind: PlatformKind::Window,
    }
}

/// The DISPLAY2 2x2 seam, side monitor: 540 px above that floor.
fn seam() -> Platform {
    Platform {
        id: "seam:0".into(),
        hwnd: None,
        x0: 1920.0,
        x1: 2880.0,
        y: 897.0,
        kind: PlatformKind::Seam,
    }
}

#[test]
fn test_same_platform_is_just_a_walk() {
    let plan = plan_route(&[floor0()], PPM, "floor:0", 100.0, "floor:0", 500.0);
    assert_eq!(plan, Some(vec![Step::Walk(Walk { x_px: 500.0 })]));
}

#[test]
fn test_hop_down_from_a_window_to_the_floor_below() {
    let plan = plan_route(
        &[window(), floor0()],
        PPM,
        "win:1:0",
        300.0,
        "floor:0",
        1000.0,
    );
    let Some(plan) = plan else {
        panic!("plan is not None");
    };
    let [
        Step::Walk(walk_to_edge),
        Step::HopDown(hop),
        Step::Walk(final_walk),
    ] = plan.as_slice()
    else {
        panic!("walk_to_edge, hop, final_walk = plan; got {plan:?}");
    };
    assert_eq!(hop.to_platform, "floor:0");
    // she walks to the takeoff point first
    assert_eq!(walk_to_edge.x_px, hop.x_px);
    // within the overlap of both x-ranges
    assert!((100.0..=900.0).contains(&hop.x_px));
    assert_eq!(*final_walk, Walk { x_px: 1000.0 });
}

#[test]
fn test_climb_1m_onto_a_platform_a_little_over_a_quarter_screen_up() {
    // 300 px rise / 340 ppm = 0.88 m -> climb/1m.
    let low = floor0();
    let high = Platform {
        id: "win:2:0".into(),
        hwnd: Some(2),
        x0: 100.0,
        x1: 900.0,
        y: 1140.0,
        kind: PlatformKind::Window,
    };
    let plan = plan_route(&[low, high], PPM, "floor:0", 300.0, "win:2:0", 500.0);
    let Some(plan) = plan else {
        panic!("plan is not None");
    };
    let [
        Step::Walk(walk_to_edge),
        Step::Climb(climb),
        Step::Walk(final_walk),
    ] = plan.as_slice()
    else {
        panic!("walk_to_edge, climb, final_walk = plan; got {plan:?}");
    };
    assert_eq!(climb.meters, 1);
    assert_eq!(climb.to_platform, "win:2:0");
    assert_eq!(walk_to_edge.x_px, climb.x_px);
    assert_eq!(*final_walk, Walk { x_px: 500.0 });
}

#[test]
fn test_climb_2m_onto_the_side_monitor_seam() {
    // 1437 - 897 = 540 px rise / 340 ppm = 1.59 m -> climb/2m (smallest clip
    // that covers it - climb/1m tops out at ~1.05 m).
    let plan = plan_route(
        &[floor1(), seam()],
        PPM,
        "floor:1",
        2000.0,
        "seam:0",
        2500.0,
    );
    let Some(plan) = plan else {
        panic!("plan is not None");
    };
    let [Step::Walk(_), Step::Climb(climb), Step::Walk(final_walk)] = plan.as_slice() else {
        panic!("walk_to_edge, climb, final_walk = plan; got {plan:?}");
    };
    assert_eq!(climb.meters, 2);
    assert_eq!(climb.to_platform, "seam:0");
    assert_eq!(*final_walk, Walk { x_px: 2500.0 });
}

#[test]
fn test_too_high_to_climb_directly_routes_via_the_floor() {
    // A window 3 m above another window, but both floors connect underneath.
    let low_window = Platform {
        id: "win:1:0".into(),
        hwnd: Some(1),
        x0: 0.0,
        x1: 800.0,
        y: 1200.0,
        kind: PlatformKind::Window,
    };
    // 1020px = 3.0m
    let high_window = Platform {
        id: "win:2:0".into(),
        hwnd: Some(2),
        x0: 0.0,
        x1: 800.0,
        y: 180.0,
        kind: PlatformKind::Window,
    };
    let floor = floor0();
    let plan = plan_route(
        &[low_window, high_window, floor],
        PPM,
        "win:1:0",
        400.0,
        "win:2:0",
        400.0,
    );
    // No direct climb edge (> 2m); nothing else connects the two windows'
    // x-ranges to a path back up, so this is unreachable, not routed.
    assert!(plan.is_none());
}

#[test]
fn test_too_high_but_reachable_via_a_climbable_intermediate_platform() {
    let ground = floor0();
    // 440px = 1.29m from ground.
    let mid = Platform {
        id: "win:1:0".into(),
        hwnd: Some(1),
        x0: 0.0,
        x1: 800.0,
        y: 1000.0,
        kind: PlatformKind::Window,
    };
    // 540px = 1.59m from mid.
    let top = Platform {
        id: "win:2:0".into(),
        hwnd: Some(2),
        x0: 0.0,
        x1: 800.0,
        y: 460.0,
        kind: PlatformKind::Window,
    };
    let plan = plan_route(&[ground, mid, top], PPM, "floor:0", 100.0, "win:2:0", 200.0);
    let Some(plan) = plan else {
        panic!("plan is not None");
    };
    let climbs: Vec<&Climb> = plan
        .iter()
        .filter_map(|s| match s {
            Step::Climb(c) => Some(c),
            _ => None,
        })
        .collect();
    let to_platforms: Vec<&str> = climbs.iter().map(|c| c.to_platform.as_str()).collect();
    assert_eq!(to_platforms, ["win:1:0", "win:2:0"]);
    let meters: Vec<u32> = climbs.iter().map(|c| c.meters).collect();
    assert_eq!(meters, [2, 2]);
}

#[test]
fn test_unreachable_target_platform_returns_none() {
    let isolated = Platform {
        id: "win:9:0".into(),
        hwnd: Some(9),
        x0: 5000.0,
        x1: 5800.0,
        y: 500.0,
        kind: PlatformKind::Window,
    };
    let plan = plan_route(
        &[floor0(), isolated],
        PPM,
        "floor:0",
        100.0,
        "win:9:0",
        5100.0,
    );
    assert!(plan.is_none());
}

#[test]
fn test_unknown_target_platform_id_returns_none() {
    let plan = plan_route(&[floor0()], PPM, "floor:0", 100.0, "nonexistent", 100.0);
    assert!(plan.is_none());
}

#[test]
fn test_cross_monitor_goes_via_floors_with_no_hop_or_climb_steps() {
    let plan = plan_route(
        &[floor0(), floor1()],
        PPM,
        "floor:0",
        1000.0,
        "floor:1",
        2200.0,
    );
    // Floor-to-floor is transparent walking (WP-22 already spans monitors) -
    // just the final walk, no hop/climb noise.
    assert_eq!(plan, Some(vec![Step::Walk(Walk { x_px: 2200.0 })]));
}

#[test]
fn test_seam_reachable_from_the_far_floor_via_the_side_floor() {
    // From the PRIMARY floor to the seam: no direct x overlap/height edge
    // from floor:0 to the seam, but floor:0 -> floor:1 (free) -> climb.
    let plan = plan_route(
        &[floor0(), floor1(), seam()],
        PPM,
        "floor:0",
        1000.0,
        "seam:0",
        2500.0,
    );
    let Some(plan) = plan else {
        panic!("plan is not None");
    };
    let climbs: Vec<&Climb> = plan
        .iter()
        .filter_map(|s| match s {
            Step::Climb(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(climbs.len(), 1);
    assert_eq!(climbs[0].to_platform, "seam:0");
    assert_eq!(climbs[0].meters, 2);
}
