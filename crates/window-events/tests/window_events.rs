//! WP-23 slice 4: window_events's pure logic (ride detection, fall-cause
//! classification, fullscreen detection, the retreat target and the annoyed-
//! line pool) plus its async orchestrators against fakes.
//!
//! Port of ref/felt-animation/tests/test_window_events.py. The two
//! `loops._sync_platform_support` tests from that file are not ported: they
//! exercise loops.py's support-sync glue (a different module, with its own
//! order), not window_events itself.

use std::collections::BTreeSet;

use felt_window_events::{
    Behavior, Clock, Foreground, Geometry, Monitor, Navigator, Overlay, OverlayMessage, Plan,
    Platform, RandomLike, WINDOW_ANNOYED_LINES, Win, WindowEventState, classify_fall_cause,
    detect_ride, is_fullscreen_on_monitor, maybe_retreat_from_fullscreen, pick_annoyed_line,
    pick_retreat_target, react_to_landing, try_ride,
};

// -- tiny executor (test fakes complete immediately, no runtime needed) ------

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(output) => return output,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

// -- fixtures ----------------------------------------------------------------

fn mon0() -> Monitor {
    Monitor {
        index: 0,
        left: 0.0,
        top: 0.0,
        right: 1920.0,
        bottom: 1080.0,
        work_bottom: Some(1040.0),
    }
}

fn mon1() -> Monitor {
    Monitor {
        index: 1,
        left: 1920.0,
        top: 0.0,
        right: 3840.0,
        bottom: 1080.0,
        work_bottom: Some(1080.0),
    }
}

fn ledge() -> Platform {
    Platform {
        id: "win:1:0".to_string(),
        hwnd: Some(1),
        x0: 100.0,
        x1: 900.0,
        y: 600.0,
        kind: "window".to_string(),
    }
}

fn floor0() -> Platform {
    Platform {
        id: "floor:0".to_string(),
        hwnd: None,
        x0: 0.0,
        x1: 1920.0,
        y: 1040.0,
        kind: "floor".to_string(),
    }
}

fn floor1() -> Platform {
    Platform {
        id: "floor:1".to_string(),
        hwnd: None,
        x0: 1920.0,
        x1: 3840.0,
        y: 1080.0,
        kind: "floor".to_string(),
    }
}

fn mk_win(hwnd: isize, rect: (f64, f64, f64, f64), title: &str) -> Win {
    Win {
        hwnd,
        rect,
        title: title.to_string(),
    }
}

// -- detect_ride ----------------------------------------------------------------

#[test]
fn test_ride_applies_dx_and_stays_on_the_platform() {
    let old_win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    let new_win = mk_win(1, (300.0, 620.0, 1100.0, 920.0), "Notepad"); // dragged +200/+20
    let result = detect_ride(Some(&ledge()), &[old_win], &[new_win], 500.0);
    assert_eq!(result, Some((200.0, 620.0)));
}

#[test]
fn test_ride_off_the_end_returns_none() {
    let old_win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    let new_win = mk_win(1, (2000.0, 600.0, 2800.0, 900.0), "Notepad"); // far away
    // her_x_px=500 + dx(1900) = 2400, inside [2000,2800] -- so pick a case
    // that lands OUTSIDE the new range instead (she's ridden off the edge):
    let result = detect_ride(Some(&ledge()), &[old_win], &[new_win], 50.0); // 50+1900=1950 < 2000
    assert!(result.is_none());
}

#[test]
fn test_ride_no_movement_is_not_a_ride() {
    let win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    assert!(
        detect_ride(
            Some(&ledge()),
            std::slice::from_ref(&win),
            std::slice::from_ref(&win),
            500.0
        )
        .is_none()
    );
}

#[test]
fn test_ride_none_when_not_on_a_window_platform() {
    let floor = floor0();
    let win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    assert!(
        detect_ride(
            Some(&floor),
            std::slice::from_ref(&win),
            std::slice::from_ref(&win),
            500.0
        )
        .is_none()
    );
}

#[test]
fn test_ride_none_when_hwnd_missing_from_either_snapshot() {
    let new_win = mk_win(1, (300.0, 620.0, 1100.0, 920.0), "Notepad");
    assert!(detect_ride(Some(&ledge()), &[], &[new_win], 500.0).is_none());
    let old_win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    assert!(detect_ride(Some(&ledge()), &[old_win], &[], 500.0).is_none());
}

// -- classify_fall_cause ----------------------------------------------------------

#[test]
fn test_cause_vanished_when_hwnd_is_gone() {
    assert_eq!(classify_fall_cause(&ledge(), &[], &[mon0()]), "vanished");
}

#[test]
fn test_cause_maximised_when_hwnd_covers_the_work_area() {
    let maximised = mk_win(1, (0.0, 0.0, 1920.0, 1040.0), "Notepad");
    assert_eq!(
        classify_fall_cause(&ledge(), &[maximised], &[mon0()]),
        "maximised"
    );
}

#[test]
fn test_cause_moved_away_when_hwnd_is_still_there_but_not_maximised() {
    let moved = mk_win(1, (2000.0, 600.0, 2800.0, 900.0), "Notepad");
    assert_eq!(
        classify_fall_cause(&ledge(), &[moved], &[mon0(), mon1()]),
        "moved_away"
    );
}

#[test]
fn test_cause_vanished_for_a_non_window_platform() {
    let floor = floor0();
    assert_eq!(classify_fall_cause(&floor, &[], &[mon0()]), "vanished");
}

// -- is_fullscreen_on_monitor -----------------------------------------------------

#[test]
fn test_fullscreen_true_when_foreground_covers_the_full_monitor_incl_taskbar() {
    let foreground: Foreground = (7, (0.0, 0.0, 1920.0, 1080.0));
    assert!(is_fullscreen_on_monitor(Some(foreground), &mon0()));
}

#[test]
fn test_fullscreen_false_for_a_merely_maximised_window() {
    // Covers the WORK area only (1040), not the full monitor bounds (1080).
    let foreground: Foreground = (7, (0.0, 0.0, 1920.0, 1040.0));
    assert!(!is_fullscreen_on_monitor(Some(foreground), &mon0()));
}

#[test]
fn test_fullscreen_false_with_no_foreground() {
    assert!(!is_fullscreen_on_monitor(None, &mon0()));
}

// -- pick_retreat_target ----------------------------------------------------------

#[test]
fn test_retreat_target_picks_the_other_monitor_floor_corner() {
    let platforms = [floor0(), floor1()];
    let monitors = [mon0(), mon1()];
    let Some((floor, corner_x)) = pick_retreat_target(&platforms, &monitors, 0) else {
        panic!("target is not None");
    };
    assert_eq!(floor.id, "floor:1");
    assert!(floor.x0 < corner_x && corner_x < floor.x1);
}

#[test]
fn test_retreat_target_none_with_a_single_monitor() {
    let platforms = [floor0()];
    let monitors = [mon0()];
    assert!(pick_retreat_target(&platforms, &monitors, 0).is_none());
}

// -- pick_annoyed_line ------------------------------------------------------------

/// Deterministic stand-in for the `random` module: round-robins choices.
#[derive(Default)]
struct FakeRng {
    counter: usize,
}

impl RandomLike for FakeRng {
    fn choice_index(&mut self, len: usize) -> usize {
        let idx = self.counter % len.max(1);
        self.counter += 1;
        idx
    }
}

#[test]
fn test_annoyed_line_never_immediately_repeats() {
    let mut rng = FakeRng::default();
    for last in WINDOW_ANNOYED_LINES {
        for _ in 0..10 {
            assert_ne!(pick_annoyed_line(Some(last), &mut rng), last);
        }
    }
}

#[test]
fn test_annoyed_line_picks_from_the_pool_with_no_last() {
    let mut rng = FakeRng::default();
    let line = pick_annoyed_line(None, &mut rng);
    assert!(WINDOW_ANNOYED_LINES.contains(&line));
}

// -- try_ride (async orchestrator) -------------------------------------------------

struct FakeOverlay {
    sent: Vec<OverlayMessage>,
}

impl Overlay for FakeOverlay {
    async fn send(&mut self, message: OverlayMessage) -> bool {
        self.sent.push(message);
        true
    }
}

/// Identity px<->world map at 340 px/m - the Python tests' _FakeGeometry.
struct FakeGeometry;

impl Geometry for FakeGeometry {
    fn world_to_px_x(&self, world_x: f64) -> f64 {
        world_x
    }
    fn px_to_world_x(&self, x_px: f64) -> f64 {
        x_px
    }
    fn ppm(&self) -> f64 {
        340.0
    }
}

fn make_state(current_platform_id: &str, current_x: f64) -> WindowEventState {
    WindowEventState {
        current_platform_id: Some(current_platform_id.to_string()),
        current_x,
        ..Default::default()
    }
}

#[test]
fn test_try_ride_sends_shift_and_updates_current_x_no_support_fall() {
    let mut overlay = FakeOverlay { sent: vec![] };
    let geometry = FakeGeometry;
    let mut state = make_state("win:1:0", 500.0);
    let old_win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    let new_win = mk_win(1, (300.0, 620.0, 1100.0, 920.0), "Notepad");

    let rode = block_on(try_ride(
        &mut state,
        &mut overlay,
        &geometry,
        false,
        &[ledge()],
        &[old_win],
        &[new_win],
    ));

    assert!(rode);
    assert_eq!(
        overlay.sent,
        vec![OverlayMessage::Shift {
            dx_px: 200.0,
            y_px: 620.0
        }]
    );
    assert_eq!(state.current_x, 700.0); // 500 + 200, identity px<->world map
}

#[test]
fn test_try_ride_returns_false_when_not_riding() {
    let mut overlay = FakeOverlay { sent: vec![] };
    let geometry = FakeGeometry;
    let mut state = make_state("win:1:0", 500.0);
    let win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");

    // no movement
    let rode = block_on(try_ride(
        &mut state,
        &mut overlay,
        &geometry,
        false,
        &[ledge()],
        std::slice::from_ref(&win),
        std::slice::from_ref(&win),
    ));

    assert!(!rode);
    assert!(overlay.sent.is_empty());
}

#[test]
fn test_try_ride_skipped_while_base_busy() {
    let mut overlay = FakeOverlay { sent: vec![] };
    let geometry = FakeGeometry;
    let mut state = make_state("win:1:0", 500.0);
    let old_win = mk_win(1, (100.0, 600.0, 900.0, 900.0), "Notepad");
    let new_win = mk_win(1, (300.0, 620.0, 1100.0, 920.0), "Notepad");

    let rode = block_on(try_ride(
        &mut state,
        &mut overlay,
        &geometry,
        true,
        &[ledge()],
        &[old_win],
        &[new_win],
    ));

    assert!(!rode);
    assert!(overlay.sent.is_empty());
}

// -- react_to_landing ---------------------------------------------------------------

/// behavior.py's run_sequence/play_presence_line + event_log.report_event
/// against recorders, like the Python tests' monkeypatches.
#[derive(Default)]
struct FakeBehavior {
    played_tags: Vec<Vec<String>>,
    lines: Vec<String>,
}

impl Behavior for FakeBehavior {
    fn start_run_sequence(&mut self, tags: Vec<String>) {
        self.played_tags.push(tags);
    }
    fn play_presence_line(&mut self, line: &str) -> bool {
        self.lines.push(line.to_string());
        true // the Python fakes return True (a curated wav "played")
    }
    fn report_event(&mut self, _kind: &str, _data: &[(&str, &str)]) {
        // WP-26: react_to_landing's own report_event("window_fall") no-ops
        // with an empty felt_core_url - no network access needed here.
    }
}

/// `time.monotonic` at a fixed reading - the Python tests' _FakeTime.
struct FakeClock(f64);

impl Clock for FakeClock {
    fn monotonic(&self) -> f64 {
        self.0
    }
}

fn react_state() -> WindowEventState {
    WindowEventState {
        known_tags: BTreeSet::from([
            "emotion/annoyed".to_string(),
            "emotion/surprised".to_string(),
        ]),
        window_event_speech_enabled: true,
        window_event_last_line: None,
        window_event_last_spoken_at: 0.0,
        ..Default::default()
    }
}

#[test]
fn test_vanished_landing_plays_annoyed_and_a_budgeted_line() {
    let mut behavior = FakeBehavior::default();
    let mut state = react_state();
    let clock = FakeClock(1000.0);
    let mut rng = FakeRng::default();

    block_on(react_to_landing(
        &mut state,
        "vanished",
        &clock,
        &mut rng,
        &mut behavior,
    ));

    assert_eq!(
        behavior.played_tags,
        vec![vec!["emotion/annoyed".to_string()]]
    );
    assert_eq!(behavior.lines.len(), 1);
    assert!(WINDOW_ANNOYED_LINES.contains(&behavior.lines[0].as_str()));
    assert_eq!(
        state.window_event_last_line.as_deref(),
        Some(behavior.lines[0].as_str())
    );
}

#[test]
fn test_maximised_landing_is_non_verbal_only() {
    let mut behavior = FakeBehavior::default();
    let mut state = react_state();
    let clock = FakeClock(1000.0);
    let mut rng = FakeRng::default();

    block_on(react_to_landing(
        &mut state,
        "maximised",
        &clock,
        &mut rng,
        &mut behavior,
    ));

    assert_eq!(
        behavior.played_tags,
        vec![vec!["emotion/surprised".to_string()]]
    );
    assert!(behavior.lines.is_empty());
}

#[test]
fn test_speech_respects_the_cooldown_after_a_recent_line() {
    let mut behavior = FakeBehavior::default();
    let mut state = react_state();
    state.window_event_last_spoken_at = 990.0; // 10s ago < 45s cooldown
    let clock = FakeClock(1000.0);
    let mut rng = FakeRng::default();

    block_on(react_to_landing(
        &mut state,
        "vanished",
        &clock,
        &mut rng,
        &mut behavior,
    ));

    assert!(behavior.lines.is_empty()); // skipped: still inside the cooldown
}

#[test]
fn test_speech_disabled_by_config_plays_no_line() {
    let mut behavior = FakeBehavior::default();
    let mut state = react_state();
    state.window_event_speech_enabled = false;
    let clock = FakeClock(1000.0);
    let mut rng = FakeRng::default();

    block_on(react_to_landing(
        &mut state,
        "vanished",
        &clock,
        &mut rng,
        &mut behavior,
    ));

    assert!(behavior.lines.is_empty());
}

// -- maybe_retreat_from_fullscreen --------------------------------------------------

/// navigation.plan_route -> a canned plan; nav_executor.run_plan records
/// that it ran - the Python tests' fake_run_plan/fake_plan_route.
#[derive(Default)]
struct FakeNavigator {
    plans_run: Vec<String>,
}

impl Navigator for FakeNavigator {
    fn plan_route(
        &self,
        _platforms: &[Platform],
        _ppm: f64,
        _current_platform_id: &str,
        _current_x_px: f64,
        _target_platform_id: &str,
        _target_x_px: f64,
    ) -> Option<Plan> {
        Some(Plan(vec!["a-plan".to_string()]))
    }
    fn start_run_plan(&mut self, _plan: Plan) {
        self.plans_run.push("ran".to_string());
    }
}

#[test]
fn test_fullscreen_on_her_monitor_relocates_once_per_episode() {
    let platforms = [floor0(), floor1()];
    let monitors = [mon0(), mon1()];
    let geometry = FakeGeometry;
    let mut state = make_state("floor:0", 500.0);
    let mut nav = FakeNavigator::default();
    let foreground: Option<Foreground> = Some((7, (0.0, 0.0, 1920.0, 1080.0))); // covers monitor 0 fully

    for _ in 0..3 {
        // simulate 3 consecutive world ticks, still fullscreen
        block_on(maybe_retreat_from_fullscreen(
            &mut state, &geometry, false, &mut nav, &platforms, &monitors, foreground,
        ));
    }

    assert_eq!(nav.plans_run, vec!["ran"]); // only the first tick planned/ran a retreat
    assert!(state.fullscreen_retreat_active);
}

#[test]
fn test_fullscreen_with_a_single_monitor_does_nothing() {
    let platforms = [floor0()];
    let monitors = [mon0()];
    let geometry = FakeGeometry;
    let mut state = make_state("floor:0", 500.0);
    let mut nav = FakeNavigator::default();
    let foreground: Option<Foreground> = Some((7, (0.0, 0.0, 1920.0, 1080.0)));

    block_on(maybe_retreat_from_fullscreen(
        &mut state, &geometry, false, &mut nav, &platforms, &monitors, foreground,
    ));

    assert!(nav.plans_run.is_empty());
    assert!(!state.fullscreen_retreat_active);
}

#[test]
fn test_fullscreen_ending_resets_the_episode_latch() {
    let platforms = [floor0(), floor1()];
    let monitors = [mon0(), mon1()];
    let geometry = FakeGeometry;
    let mut state = make_state("floor:0", 500.0);
    state.fullscreen_retreat_active = true;
    let mut nav = FakeNavigator::default();

    block_on(maybe_retreat_from_fullscreen(
        &mut state, &geometry, false, &mut nav, &platforms, &monitors,
        None, // no foreground -> not fullscreen anymore
    ));

    assert!(!state.fullscreen_retreat_active);
}
