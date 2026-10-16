//! WP-30: hobbies while Mario is AFK.

use std::collections::HashMap;

use felt_hobbies::{
    HobbyDirector, HobbyEntry, HobbyState, ManualClock, Rng, VISIBLE, hobby_weights, nap_face, pick,
};

/// Deterministic stand-in for `random.Random(0)`: every draw is the low end.
struct TestRng;

impl Rng for TestRng {
    fn uniform(&mut self, lo: f64, _hi: f64) -> f64 {
        lo
    }
    fn index_below(&mut self, _n: usize) -> usize {
        0
    }
}

fn mood(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn test_weights_follow_hour_mood_and_clutter() {
    let night = hobby_weights(2, &mood(&[("energy", 0.2)]), false, None);
    let day = hobby_weights(15, &mood(&[("energy", 0.9)]), false, None);
    assert!(night["napping"] > day["napping"]);
    assert!(day["gaming"] > night["gaming"]);
    assert!(night["tidying"] == 0.0);
    assert!(hobby_weights(15, &HashMap::new(), true, None)["tidying"] > 0.0);
    assert!(hobby_weights(15, &HashMap::new(), false, Some("reading"))["reading"] == 0.0);
}

#[test]
fn test_pick_never_returns_zero_weight() {
    let mut rng = TestRng;
    let both = HashMap::from([("a".to_string(), 0.0), ("b".to_string(), 1.0)]);
    for _ in 0..50 {
        assert_eq!(pick(&both, &mut rng).as_deref(), Some("b"));
    }
    let only_zero = HashMap::from([("a".to_string(), 0.0)]);
    assert!(pick(&only_zero, &mut rng).is_none());
}

/// The Python tests' `FakeProps` + `_state()`: a visible presence, props that
/// log every call, a no-op `hold_base`, and an overlay that records faces.
struct FakeState {
    presence: String,
    log: Vec<String>,
    faces: Vec<HashMap<String, f64>>,
    reported: Vec<(String, HobbyEntry)>,
}

fn fake_state() -> FakeState {
    FakeState {
        presence: VISIBLE.to_string(),
        log: Vec::new(),
        faces: Vec::new(),
        reported: Vec::new(),
    }
}

impl HobbyState for FakeState {
    fn presence_state(&self) -> &str {
        &self.presence
    }
    fn mood(&self) -> HashMap<String, f64> {
        mood(&[("energy", 0.9)])
    }
    fn has_known_tag(&self, _tag: &str) -> bool {
        false // known_tags = set()
    }
    fn props_enabled(&self) -> bool {
        true
    }
    fn tidy_list(&self) -> Vec<String> {
        Vec::new()
    }
    fn tidy_one(&mut self) -> bool {
        self.log.push("tidy".to_string());
        true
    }
    fn sit_down(&mut self) -> bool {
        self.log.push("sit".to_string());
        true
    }
    fn conjure_held(&mut self, item: &str, _bone: &str) -> String {
        self.log.push(format!("conjure:{item}"));
        "p1".to_string()
    }
    fn dismiss(&mut self, pid: &str) {
        self.log.push(format!("dismiss:{pid}"));
    }
    fn stand_up_if_sitting(&mut self) {
        self.log.push("stand".to_string());
    }
    fn use_item(&mut self, item: &str, _place: bool) -> bool {
        self.log.push(format!("place:{item}"));
        true
    }
    fn hold_base_enter(&mut self) {}
    fn hold_base_exit(&mut self) {}
    fn resting_face(&mut self, shapes: &HashMap<String, f64>) -> bool {
        self.faces.push(shapes.clone());
        true
    }
    fn run_sequence(&mut self, _tags: &[String]) {}
    fn update_resting_face(&mut self, _force: bool) {
        // The Python test monkeypatches `behavior.update_resting_face` to a
        // no-op; nothing in the assertions observes it.
    }
    fn report_event(&mut self, kind: &str, data: &HobbyEntry) {
        self.reported.push((kind.to_string(), data.clone()));
    }
}

/// The monkeypatched `hobbies.hobby_weights` -> `{"reading": 1.0}`.
fn only_reading(
    _hour: i32,
    _mood: &HashMap<String, f64>,
    _can_tidy: bool,
    _last: Option<&str>,
) -> HashMap<String, f64> {
    HashMap::from([("reading".to_string(), 1.0)])
}

/// The monkeypatched `hobbies.hobby_weights` -> `{"napping": 1.0}`.
fn only_napping(
    _hour: i32,
    _mood: &HashMap<String, f64>,
    _can_tidy: bool,
    _last: Option<&str>,
) -> HashMap<String, f64> {
    HashMap::from([("napping".to_string(), 1.0)])
}

#[test]
fn test_reading_then_stop_cleans_up_and_reports() {
    let state = fake_state();
    let mut d = HobbyDirector::new(state, TestRng, ManualClock::new(0.0, 12));
    d.set_weights_fn(only_reading);
    d.set_duration_bounds(60.0, 60.0); // monkeypatched MIN_S / MAX_S
    d.start();
    assert!(d.active());
    // The task runs to its sleep(`duration`); the test's clock stays put.
    let _ = d.run_until_blocked();
    assert_eq!(d.current(), Some("reading"));
    assert_eq!(d.state().log, ["sit", "conjure:book"]);
    d.clock_mut().set_now(125.0); // two minutes in, Mario comes back
    let done = d.stop();
    assert!(!d.active());
    let log = &d.state().log;
    assert_eq!(log[log.len() - 2..].to_vec(), ["dismiss:p1", "stand"]);
    assert_eq!(
        done,
        [HobbyEntry {
            hobby: "reading".to_string(),
            minutes: 2
        }]
    );
    assert_eq!(
        d.state().reported,
        [(
            "hobby".to_string(),
            HobbyEntry {
                hobby: "reading".to_string(),
                minutes: 2
            }
        )]
    );
}

#[test]
fn test_napping_lays_out_the_bed_and_closes_her_eyes() {
    let state = fake_state();
    let mut d = HobbyDirector::new(state, TestRng, ManualClock::new(0.0, 12));
    d.set_weights_fn(only_napping);
    d.start();
    let _ = d.run_until_blocked();
    assert_eq!(d.state().log, ["place:blanket", "place:pillow", "sit"]);
    assert_eq!(d.state().faces, [nap_face()]);
    d.stop();
    let log = &d.state().log;
    assert_eq!(log[log.len() - 1], "stand");
}

#[test]
fn test_no_start_while_hidden() {
    let mut state = fake_state();
    state.presence = "hidden".to_string();
    let mut d = HobbyDirector::new(state, TestRng, ManualClock::new(0.0, 12));
    d.start();
    assert!(!d.active());
}
