//! ledges_near: the "force her onto a ledge" picker.

use felt_navigation::{LEDGE_MIN_WIDTH_PX, Platform, PlatformKind, ledges_near};

fn floor() -> Platform {
    Platform {
        id: "floor:0".into(),
        hwnd: None,
        x0: 0.0,
        x1: 3440.0,
        y: 1440.0,
        kind: PlatformKind::Floor,
    }
}

fn near() -> Platform {
    Platform {
        id: "win:1:0".into(),
        hwnd: Some(1),
        x0: 2000.0,
        x1: 3000.0,
        y: 400.0,
        kind: PlatformKind::Window,
    }
}

fn far() -> Platform {
    Platform {
        id: "win:2:0".into(),
        hwnd: Some(2),
        x0: 100.0,
        x1: 900.0,
        y: 800.0,
        kind: PlatformKind::Window,
    }
}

fn narrow() -> Platform {
    Platform {
        id: "win:3:0".into(),
        hwnd: Some(3),
        x0: 2400.0,
        x1: 2480.0,
        y: 390.0,
        kind: PlatformKind::Window,
    }
}

fn seam() -> Platform {
    Platform {
        id: "win:4:0".into(),
        hwnd: Some(4),
        x0: 3440.0,
        x1: 4400.0,
        y: 897.0,
        kind: PlatformKind::Window,
    }
}

#[test]
fn test_nearest_ledge_to_the_cursor_first_and_floors_skipped() {
    let out = ledges_near(
        &[floor(), far(), near(), seam()],
        (2500.0, 420.0),
        None,
        LEDGE_MIN_WIDTH_PX,
    );
    let ids: Vec<&str> = out.iter().map(|(p, _)| p.id.as_str()).collect();
    assert_eq!(ids, ["win:1:0", "win:4:0", "win:2:0"]);
    // cursor x is inside the ledge
    assert_eq!(out[0].1, 2500.0);
}

#[test]
fn test_landing_x_is_clamped_away_from_the_ledge_ends() {
    let out = ledges_near(&[near()], (5000.0, 400.0), None, LEDGE_MIN_WIDTH_PX);
    let Some(&(_, x)) = out.first() else {
        panic!("(_, x), *_ = out");
    };
    assert_eq!(x, near().x1 - 60.0);
}

#[test]
fn test_too_narrow_and_her_own_ledge_are_skipped() {
    let out = ledges_near(
        &[near(), narrow(), far()],
        (2440.0, 390.0),
        Some("win:1:0"),
        LEDGE_MIN_WIDTH_PX,
    );
    let ids: Vec<&str> = out.iter().map(|(p, _)| p.id.as_str()).collect();
    assert_eq!(ids, ["win:2:0"]);
}
