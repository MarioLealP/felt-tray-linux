//! WP-32: worn accessories - one per slot, outfit beats ambient.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use serde_json::{Value, json};

use felt_accessories::{
    AccessoryDirector, AccessoryState, Clock, SeededRng, WornItem, WornSummary,
};

fn manifest() -> Value {
    json!({
        "items": {
            "witch_hat": {"grip": {"Head": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
            "santa_hat": {"grip": {"Head": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
            "cap": {"grip": {"Head": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
            "cat_ears": {"grip": {"Head": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
            "backpack": {"grip": {"Chest": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
            "crossbody_bag": {"grip": {"Hips": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
            "round_glasses": {"grip": {"Head": {"pos": [0, 0, 0], "rot": [0, 0, 0]}}},
        },
        "accessories": {
            "slots": {
                "hat": ["witch_hat", "santa_hat", "cap", "cat_ears"],
                "face": ["round_glasses"],
                "bag": ["backpack", "crossbody_bag"],
            },
            "tags": {
                "cap": ["summer"],
                "cat_ears": ["cute"],
                "round_glasses": ["evening"],
                "witch_hat": ["halloween"],
            },
        },
    })
}

struct FakeProps {
    log: Vec<String>,
    n: i32,
    enabled: bool,
    manifest: Value,
}

impl FakeProps {
    fn new() -> Self {
        Self {
            log: Vec::new(),
            n: 0,
            enabled: true,
            manifest: manifest(),
        }
    }
}

struct FakeState {
    props: FakeProps,
    mood: HashMap<String, f64>,
    weather: Option<String>,
}

impl AccessoryState for FakeState {
    type Error = std::convert::Infallible;

    fn props_enabled(&self) -> bool {
        self.props.enabled
    }

    fn manifest(&self) -> &Value {
        &self.props.manifest
    }

    fn mood(&self) -> &HashMap<String, f64> {
        &self.mood
    }

    fn weather_condition(&self) -> Option<&str> {
        self.weather.as_deref()
    }

    fn conjure_held(&mut self, item: &str, bone: &str) -> Result<String, Self::Error> {
        self.props.n += 1;
        self.props.log.push(format!("+{item}@{bone}"));
        Ok(format!("{item}-{}", self.props.n))
    }

    fn dismiss(&mut self, pid: &str) -> Result<(), Self::Error> {
        self.props.log.push(format!("-{pid}"));
        Ok(())
    }

    fn report_event(&mut self, _kind: &str, _data: &Value) {
        // monkeypatched to a no-op in the Python tests
    }
}

struct FakeClock {
    now: Rc<Cell<f64>>,
    hour: i32,
}

impl FakeClock {
    fn new(now: f64) -> Self {
        Self {
            now: Rc::new(Cell::new(now)),
            hour: 12,
        }
    }
}

impl Clock for FakeClock {
    fn monotonic(&self) -> f64 {
        self.now.get()
    }

    fn hour(&self) -> i32 {
        self.hour
    }
}

type Director = AccessoryDirector<FakeState, SeededRng, FakeClock>;

fn _director(mood: Option<HashMap<String, f64>>) -> Director {
    let state = FakeState {
        props: FakeProps::new(),
        mood: mood.unwrap_or_default(),
        weather: None,
    };
    AccessoryDirector::new(state, SeededRng::new(0), FakeClock::new(0.0)) // random.Random(0)
}

#[test]
fn test_hat_and_bag_combine_but_two_bags_or_two_hats_dont() {
    let mut d = _director(None);
    assert!(d.wear("backpack", "ambient", Some(60.0)).unwrap());
    assert!(d.wear("santa_hat", "ambient", Some(60.0)).unwrap());
    assert_eq!(
        d.snapshot().into_keys().collect::<BTreeSet<String>>(),
        BTreeSet::from(["bag".to_string(), "hat".to_string()])
    );
    d.wear("crossbody_bag", "ambient", Some(60.0)).unwrap(); // replaces the backpack
    d.wear("witch_hat", "ambient", Some(60.0)).unwrap(); // replaces the santa hat
    assert_eq!(
        d.snapshot()
            .into_iter()
            .map(|(s, v)| (s, v.item))
            .collect::<HashMap<String, String>>(),
        HashMap::from([
            ("bag".to_string(), "crossbody_bag".to_string()),
            ("hat".to_string(), "witch_hat".to_string()),
        ])
    );
    assert!(d.state.props.log.contains(&"-backpack-1".to_string()));
    assert!(d.state.props.log.contains(&"-santa_hat-2".to_string()));
}

#[test]
fn test_outfit_piece_keeps_its_slot() {
    let mut d = _director(None);
    d.set_outfit(&["witch_hat".to_string()]).unwrap();
    assert!(!d.wear("cat_ears", "ambient", Some(60.0)).unwrap());
    assert_eq!(
        d.snapshot()["hat"],
        WornSummary {
            item: "witch_hat".to_string(),
            source: "outfit".to_string(),
        }
    );
    d.set_outfit(&[]).unwrap(); // outfit changed: its hat comes off
    assert!(!d.snapshot().contains_key("hat"));
}

#[test]
fn test_ambient_piece_expires() {
    let mut d = _director(None);
    d.clock.now.set(100.0); // monkeypatched _now -> clock[0]
    d.wear("round_glasses", "ambient", Some(60.0)).unwrap();
    d.last_check = d.clock.now.get(); // no new pick this tick
    d.clock.now.set(161.0); // clock[0] += 61
    d.tick().unwrap();
    assert!(!d.snapshot().contains_key("face"));
    assert!(
        d.state
            .props
            .log
            .last()
            .unwrap()
            .starts_with("-round_glasses")
    );
}

#[test]
fn test_pick_follows_tags_and_free_slots() {
    let mut d = _director(Some(HashMap::from([("happiness".to_string(), 0.9)])));
    assert_eq!(d.pick(12), Some("cat_ears".to_string())); // cute; no summer weather, not evening
    assert!(matches!(
        d.pick(20).as_deref(),
        Some("cat_ears" | "round_glasses")
    ));
    d.worn.insert(
        "hat".to_string(),
        WornItem {
            item: "cap".to_string(),
            pid: "x".to_string(),
            source: "ambient".to_string(),
            until: None,
        },
    );
    assert!(d.pick(12).is_none()); // the only cute item's slot is taken
    assert!(
        !(0..24)
            .map(|h| d.pick(h))
            .collect::<BTreeSet<Option<String>>>()
            .contains(&Some("witch_hat".to_string()))
    ); // outfit-only
}
