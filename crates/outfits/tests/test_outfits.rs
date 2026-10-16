//! WP-27 outfits: pure decision rules + director behaviour with doubles.
//! Port of `ref/felt-animation/tests/test_outfits.py`.
//!
//! `test_outfit_endpoint` is not ported: it drives the FastAPI debug router
//! (`routers/debug.py`, another module) over HTTP; see the order report.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use felt_outfits::director::{DirectorConfig, DirectorHooks};
use felt_outfits::state::{
    Behavior, HIDDEN, HoldGuard, Mood, OutfitState, Overlay, Player, Presence, PresenceRouter,
    PropsDirector, VISIBLE,
};
use felt_outfits::wardrobe::{FileWardrobe, WardrobeStore};
use felt_outfits::{
    Date, DateTime, OutfitDirector, OutfitRng, PyRandom, ambient_pick, bikini_roll, effective_date,
    load_outfits, pool_ids, scheduled_outfit,
};

// -- helpers ---------------------------------------------------------------

fn dt(m: u32, d: u32, h: u32) -> DateTime {
    DateTime::new(2026, m, d, h, 0, 0)
}

fn catalogue() -> Vec<felt_outfits::Outfit> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/outfits.json");
    load_outfits(&root)
}

fn tmp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmp dir");
    dir
}

fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(1));
    }
}

fn real_mono() -> Box<dyn Fn() -> f64 + Send + Sync> {
    let start = Instant::now();
    Box::new(move || start.elapsed().as_secs_f64())
}

/// Python's test-local `Fixed(random.Random)` - a constant `random()`.
struct FixedRng(f64);

impl OutfitRng for FixedRng {
    fn random(&mut self) -> f64 {
        self.0
    }
    fn choice_index(&mut self, _len: usize) -> usize {
        0
    }
}

// -- doubles ---------------------------------------------------------------

struct NoopHold;
impl HoldGuard for NoopHold {}

struct FakeOverlay {
    sent: Mutex<Vec<Value>>,
}

impl Overlay for FakeOverlay {
    fn send(&self, payload: &Value) -> bool {
        self.sent.lock().unwrap().push(payload.clone());
        true
    }
    fn apply_garment(&self, _material: &str, _albedo: &str) {}
    fn play_tag(&self, _tag: &str) {}
}

struct FakeBehavior {
    busy: Mutex<bool>,
    posture: Mutex<String>,
    clips: Mutex<Vec<Vec<String>>>,
}

impl Behavior for FakeBehavior {
    fn busy(&self) -> bool {
        *self.busy.lock().unwrap()
    }
    fn posture(&self) -> String {
        self.posture.lock().unwrap().clone()
    }
    fn run_sequence(&self, tags: Vec<String>, _timeout: f64) {
        self.clips.lock().unwrap().push(tags);
    }
    fn hold_base(&self) -> Box<dyn HoldGuard> {
        Box::new(NoopHold)
    }
}

struct FakePresence {
    state: Mutex<String>,
}

impl Presence for FakePresence {
    fn state(&self) -> String {
        self.state.lock().unwrap().clone()
    }
}

struct NoopPresenceRouter;
impl PresenceRouter for NoopPresenceRouter {
    fn do_hide(&self) {}
    fn do_show(&self) {}
}

struct FakeMood {
    energy: f64,
}

impl Mood for FakeMood {
    fn energy(&self) -> Option<f64> {
        Some(self.energy)
    }
}

struct FakePlayer {
    playing: Mutex<bool>,
}

impl Player for FakePlayer {
    fn is_playing(&self) -> bool {
        *self.playing.lock().unwrap()
    }
}

struct FakeProps {
    seated: Mutex<Option<String>>,
}

impl PropsDirector for FakeProps {
    fn seated_on(&self) -> Option<String> {
        self.seated.lock().unwrap().clone()
    }
}

struct Ctx {
    overlay: Arc<FakeOverlay>,
    behavior: Arc<FakeBehavior>,
    presence: Arc<FakePresence>,
    player: Arc<FakePlayer>,
    props: Arc<FakeProps>,
    ledge: Arc<Mutex<Option<String>>>,
    wardrobe: Arc<FileWardrobe>,
}

struct MakeOpts {
    energy: f64,
    chance: f64,
    seed: u64,
    tags: Vec<String>,
    when: DateTime,
}

impl Default for MakeOpts {
    fn default() -> Self {
        Self {
            energy: 0.8,
            chance: 0.0,
            seed: 1,
            tags: Vec::new(),
            when: dt(9, 15, 12),
        }
    }
}

fn make(tmp: &Path, opts: MakeOpts) -> (Arc<OutfitDirector>, Ctx) {
    let wardrobe = Arc::new(FileWardrobe::new(tmp.join("wardrobe.json"), "res://tees"));
    let overlay = Arc::new(FakeOverlay {
        sent: Mutex::new(Vec::new()),
    });
    let behavior = Arc::new(FakeBehavior {
        busy: Mutex::new(false),
        posture: Mutex::new("standing".to_string()),
        clips: Mutex::new(Vec::new()),
    });
    let presence = Arc::new(FakePresence {
        state: Mutex::new(VISIBLE.to_string()),
    });
    let player = Arc::new(FakePlayer {
        playing: Mutex::new(false),
    });
    let props = Arc::new(FakeProps {
        seated: Mutex::new(None),
    });
    let ledge: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let state = Arc::new(OutfitState {
        overlay: overlay.clone(),
        behavior: behavior.clone(),
        presence: presence.clone(),
        presence_router: Arc::new(NoopPresenceRouter),
        mood: Arc::new(FakeMood {
            energy: opts.energy,
        }),
        player: player.clone(),
        props_director: props.clone(),
        accessories: None,
        seated_on_ledge: Arc::clone(&ledge),
        known_tags: Arc::new(Mutex::new(opts.tags.clone())),
    });
    let now = opts.when;
    let hooks = DirectorHooks {
        rng: Box::new(PyRandom::from_seed(opts.seed)),
        clock: Box::new(move || now),
        mono: real_mono(),
    };
    let config = DirectorConfig {
        ambient_chance: opts.chance,
        ..Default::default()
    };
    let director = Arc::new(OutfitDirector::new(
        state,
        catalogue(),
        wardrobe.clone(),
        hooks,
        config,
    ));
    (
        director,
        Ctx {
            overlay,
            behavior,
            presence,
            player,
            props,
            ledge,
            wardrobe,
        },
    )
}

// -- pure decision rules ---------------------------------------------------

#[test]
fn test_catalogue_pools() {
    let cat = catalogue();
    assert_eq!(cat.len(), 17);
    let p = |k: &str| pool_ids(&cat, k);
    let as_set = |ids: Vec<String>| ids.into_iter().collect::<std::collections::BTreeSet<_>>();
    let expected = |ids: &[&str]| {
        ids.iter()
            .map(|s| s.to_string())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        as_set(p("themed")),
        expected(&["china", "maid", "cyberpunk", "cyberpunk2"])
    );
    assert_eq!(
        as_set(p("special")),
        expected(&["pyjama", "halloween", "xmas", "reindeer", "bikini"])
    );
    assert!(p("daily").contains(&"default".to_string()));
    assert_eq!(p("daily").len(), 8);
    let default = cat
        .iter()
        .find(|e| e.id == "default")
        .expect("default entry");
    assert_eq!(default.vrm, "res://characters/felt.vrm");
}

#[test]
fn test_daily_pick_avoids_yesterday_and_persists() {
    let dir = tmp_dir("daily_pick");
    let (d, ctx) = make(&dir, MakeOpts::default());
    let mut ws = BTreeMap::new();
    ws.insert("outfit_daily".to_string(), json!("sport"));
    ws.insert("outfit_daily_date".to_string(), json!("2026-09-14"));
    ctx.wardrobe.save(&ws);
    for seed in 0..40u64 {
        d.set_rng(Box::new(PyRandom::from_seed(seed)));
        ctx.wardrobe.save(&ws);
        assert_ne!(d.daily_pick(&dt(9, 15, 12)), "sport");
    }
    d.set_rng(Box::new(PyRandom::from_seed(3)));
    ctx.wardrobe.save(&ws);
    let first = d.daily_pick(&dt(9, 15, 12));
    d.set_rng(Box::new(PyRandom::from_seed(99)));
    assert_eq!(d.daily_pick(&dt(9, 15, 20)), first); // same day: persisted
    let saved = ctx.wardrobe.load();
    assert_eq!(saved.get("outfit_daily"), Some(&json!(first)));
    assert_eq!(saved.get("outfit_daily_prev"), Some(&json!("sport")));
}

#[test]
fn test_daily_pick_rolls_at_five_not_midnight() {
    assert_eq!(
        effective_date(&dt(9, 16, 3)),
        Date {
            year: 2026,
            month: 9,
            day: 15
        }
    );
    assert_eq!(
        effective_date(&dt(9, 16, 5)),
        Date {
            year: 2026,
            month: 9,
            day: 16
        }
    );
}

#[test]
fn test_date_and_night_rules() {
    assert_eq!(scheduled_outfit(&dt(10, 31, 12), false), Some("halloween"));
    let got: Vec<Option<&str>> = [22, 23, 24, 25, 26, 27]
        .iter()
        .map(|&d| scheduled_outfit(&dt(12, d, 12), false))
        .collect();
    assert_eq!(
        got,
        vec![
            None,
            Some("xmas"),
            Some("reindeer"),
            Some("xmas"),
            Some("reindeer"),
            None
        ]
    );
    assert_eq!(scheduled_outfit(&dt(9, 15, 0), false), None);
    assert_eq!(scheduled_outfit(&dt(9, 15, 1), false), Some("pyjama"));
    assert_eq!(scheduled_outfit(&dt(9, 15, 4), false), Some("pyjama"));
    assert_eq!(scheduled_outfit(&dt(9, 15, 5), false), None);
    assert_eq!(scheduled_outfit(&dt(9, 15, 15), true), Some("pyjama"));
}

#[test]
fn test_ambient_pool_weighting() {
    let cat = catalogue();
    let mut rng = PyRandom::from_seed(7);
    let daily = pool_ids(&cat, "daily");
    let themed = pool_ids(&cat, "themed");
    let special: std::collections::BTreeSet<String> =
        pool_ids(&cat, "special").into_iter().collect();
    let picks: Vec<String> = (0..4000)
        .map(|_| ambient_pick(&daily, &themed, &mut rng))
        .collect();
    assert!(picks.iter().all(|p| !special.contains(p)));
    let share = picks.iter().filter(|p| themed.contains(p)).count() as f64 / picks.len() as f64;
    assert!((0.12..0.18).contains(&share));
}

#[test]
fn test_bikini_only_on_sunny_weather_tag() {
    assert!(bikini_roll(
        Some("reaction/weather/sunny"),
        &mut FixedRng(0.0),
        0.01
    ));
    assert!(!bikini_roll(
        Some("reaction/weather/sunny"),
        &mut FixedRng(0.5),
        0.01
    ));
    assert!(!bikini_roll(
        Some("reaction/weather/rain"),
        &mut FixedRng(0.0),
        0.01
    ));
    assert!(!bikini_roll(
        Some("reaction/happy"),
        &mut FixedRng(0.0),
        0.01
    ));
    assert!(!bikini_roll(None, &mut FixedRng(0.0), 0.01));
}

// -- director behaviour ----------------------------------------------------

#[test]
fn test_bikini_reply_triggers_quick_change() {
    let dir = tmp_dir("bikini_reply");
    let (d, ctx) = make(&dir, MakeOpts::default());
    d.set_bikini_chance(1.0);
    Arc::clone(&d).on_reply_tag(Some("reaction/weather/rain"));
    assert_eq!(d.task_count(), 0);
    Arc::clone(&d).on_reply_tag(Some("reaction/weather/sunny"));
    let task = d.first_task().expect("bikini task spawned");
    wait_until("overlay send", || {
        !ctx.overlay.sent.lock().unwrap().is_empty()
    });
    d.on_overlay_message(&json!({"outfit_applied": {"id": "bikini"}}));
    task.join();
    let sent = ctx.overlay.sent.lock().unwrap();
    assert_eq!(
        sent[0],
        json!({"outfit": {"id": "bikini", "vrm": "res://characters/Felt_Bikini.vrm", "fx": "puff"}})
    );
    assert_eq!(d.current(), "bikini");
}

/// One way to put the director's context into a "must defer" state.
type Mutation = Box<dyn Fn(&Ctx)>;

#[test]
fn test_defer_while_seated_hidden_speaking() {
    let dir = tmp_dir("defer");
    let mutations: Vec<Mutation> = vec![
        Box::new(|ctx| *ctx.ledge.lock().unwrap() = Some("w1".to_string())),
        Box::new(|ctx| *ctx.props.seated.lock().unwrap() = Some("chair".to_string())),
        Box::new(|ctx| *ctx.behavior.posture.lock().unwrap() = "sitting".to_string()),
        Box::new(|ctx| *ctx.presence.state.lock().unwrap() = HIDDEN.to_string()),
        Box::new(|ctx| *ctx.player.playing.lock().unwrap() = true),
        Box::new(|ctx| *ctx.behavior.busy.lock().unwrap() = true),
    ];
    for mutate in &mutations {
        let (d, ctx) = make(
            &dir,
            MakeOpts {
                when: dt(10, 31, 12),
                ..Default::default()
            },
        ); // halloween wanted
        mutate(&ctx);
        assert!(!d.can_change());
        assert!(d.tick(None).is_none());
        assert!(ctx.overlay.sent.lock().unwrap().is_empty());
    }
    let (d, _ctx) = make(
        &dir,
        MakeOpts {
            when: dt(10, 31, 12),
            ..Default::default()
        },
    );
    assert!(d.can_change());
}

#[test]
fn test_quick_change_plays_spin_then_swaps() {
    let dir = tmp_dir("quick_spin");
    let (d, ctx) = make(
        &dir,
        MakeOpts {
            tags: vec!["gesture/spin".to_string(), "idle".to_string()],
            ..Default::default()
        },
    );
    d.set_quick_swap_delay(0.0);
    let worker = Arc::clone(&d);
    let handle = thread::spawn(move || worker.change_to("maid", "quick"));
    wait_until("overlay send", || {
        !ctx.overlay.sent.lock().unwrap().is_empty()
    });
    d.on_overlay_message(&json!({"outfit_applied": {"id": "maid"}}));
    assert!(handle.join().unwrap());
    assert_eq!(
        *ctx.behavior.clips.lock().unwrap(),
        vec![vec!["gesture/spin".to_string()]]
    );
    assert_eq!(
        ctx.overlay.sent.lock().unwrap()[0]["outfit"]["fx"],
        json!("puff")
    );
    assert_eq!(
        ctx.wardrobe.load().get("outfit_current"),
        Some(&json!("maid"))
    );
}

#[test]
fn test_quick_without_spin_clip_still_swaps() {
    let dir = tmp_dir("quick_no_spin");
    let (d, ctx) = make(&dir, MakeOpts::default());
    let worker = Arc::clone(&d);
    let handle = thread::spawn(move || worker.change_to("china", "quick"));
    wait_until("overlay send", || {
        !ctx.overlay.sent.lock().unwrap().is_empty()
    });
    d.on_overlay_message(&json!({"outfit_applied": {"id": "china"}}));
    assert!(handle.join().unwrap());
    assert!(ctx.behavior.clips.lock().unwrap().is_empty());
}

#[test]
fn test_failure_falls_back_to_default() {
    let dir = tmp_dir("failure_fallback");
    let (d, ctx) = make(&dir, MakeOpts::default());
    let worker = Arc::clone(&d);
    let handle = thread::spawn(move || worker.change_to("maid", "quick"));
    wait_until("overlay send", || {
        !ctx.overlay.sent.lock().unwrap().is_empty()
    });
    d.on_overlay_message(&json!({"outfit_failed": {"id": "maid", "error": "boom"}}));
    assert!(!handle.join().unwrap());
    wait_until("fallback default send", || {
        ctx.overlay.sent.lock().unwrap().len() >= 2
    });
    assert_eq!(d.current(), "default");
    let sent = ctx.overlay.sent.lock().unwrap();
    assert_eq!(
        sent[sent.len() - 1]["outfit"],
        json!({"id": "default", "vrm": "res://characters/felt.vrm", "fx": "none"})
    );
}

#[test]
fn test_reconnect_resends_current_outfit() {
    let dir = tmp_dir("reconnect");
    let (d, ctx) = make(&dir, MakeOpts::default());
    d.on_connected();
    assert!(ctx.overlay.sent.lock().unwrap().is_empty()); // default: skipped
    d.set_current("sport");
    d.on_connected();
    assert_eq!(
        *ctx.overlay.sent.lock().unwrap(),
        vec![
            json!({"outfit": {"id": "sport", "vrm": "res://characters/Felt_Sport.vrm", "fx": "none"}})
        ]
    );
    let (d2, _ctx) = make(&dir, MakeOpts::default()); // persisted across a restart
    assert_eq!(d2.current(), "sport");
}

#[test]
fn test_ambient_tick_quick_change_and_override() {
    let dir = tmp_dir("ambient_tick");
    let (d, ctx) = make(
        &dir,
        MakeOpts {
            chance: 1.0,
            seed: 2,
            ..Default::default()
        },
    );
    d.set_quick_swap_delay(0.0);
    let mut ws = BTreeMap::new();
    ws.insert("outfit_daily".to_string(), json!("sport"));
    ws.insert("outfit_daily_date".to_string(), json!("2026-09-15"));
    ctx.wardrobe.save(&ws);
    d.set_current("sport");
    let worker = Arc::clone(&d);
    let handle = thread::spawn(move || worker.tick(None));
    wait_until("overlay send", || {
        !ctx.overlay.sent.lock().unwrap().is_empty()
    });
    let sent_id = ctx.overlay.sent.lock().unwrap()[0]["outfit"]["id"]
        .as_str()
        .expect("sent id")
        .to_string();
    d.on_overlay_message(&json!({"outfit_applied": {"id": sent_id}}));
    let changed = handle.join().unwrap().expect("tick changed outfit");
    assert!(!changed.is_empty() && changed != "pyjama" && d.current() == changed);
    let override_ = d.override_value().expect("override held");
    assert_eq!(override_["id"], json!(changed.clone()));
    assert_eq!(d.target(None), changed); // held until the override lapses
}

#[test]
fn test_sleepy_target_and_night_clears_override() {
    let dir = tmp_dir("sleepy");
    let (d, _ctx) = make(
        &dir,
        MakeOpts {
            energy: 0.1,
            ..Default::default()
        },
    );
    assert_eq!(d.target(None), "pyjama");
    let (d2, _ctx) = make(
        &dir,
        MakeOpts {
            when: dt(9, 15, 2),
            ..Default::default()
        },
    );
    d2.set_override_raw(Some(json!({"id": "maid", "until": 9e12})));
    assert_eq!(d2.target(None), "pyjama");
    assert!(d2.override_value().is_none());
}

#[test]
fn test_wardrobe_tee_fields_survive() {
    let dir = tmp_dir("tee_fields");
    let (d, ctx) = make(&dir, MakeOpts::default());
    let mut ws = BTreeMap::new();
    ws.insert("current_tee".to_string(), json!("cat"));
    ws.insert("last_worn_date".to_string(), json!("x"));
    ws.insert("history".to_string(), json!({}));
    ctx.wardrobe.save(&ws);
    d.set_current("maid");
    let ws = ctx.wardrobe.load();
    assert_eq!(ws.get("current_tee"), Some(&json!("cat")));
    assert_eq!(ws.get("outfit_current"), Some(&json!("maid")));
}

#[test]
fn test_a_change_picks_the_original_or_a_kept_variant_and_reconnect_resends_it() {
    let dir = tmp_dir("variants");
    let (d, ctx) = make(
        &dir,
        MakeOpts {
            seed: 3,
            ..Default::default()
        },
    );
    let sport = d.vrms("sport");
    assert_eq!(sport[0], "res://characters/Felt_Sport.vrm");
    assert!(sport.len() > 1);
    assert!(
        sport[1..]
            .iter()
            .all(|v| v.starts_with("res://characters/variants/Felt_Sport__"))
    );

    let worker = Arc::clone(&d);
    let handle = thread::spawn(move || worker.change_to("sport", "quick"));
    wait_until("overlay send", || {
        !ctx.overlay.sent.lock().unwrap().is_empty()
    });
    let sent_vrm = ctx.overlay.sent.lock().unwrap()[0]["outfit"]["vrm"]
        .as_str()
        .expect("sent vrm")
        .to_string();
    d.on_overlay_message(&json!({"outfit_applied": {"id": "sport"}}));
    assert!(handle.join().unwrap());
    assert!(sport.contains(&sent_vrm));
    assert_eq!(d.current_vrm(), Some(sent_vrm.clone()));

    // A restarted overlay gets the same variant back, from the saved state.
    let (d2, st2) = make(&dir, MakeOpts::default());
    d2.on_connected();
    assert_eq!(
        *st2.overlay.sent.lock().unwrap(),
        vec![json!({"outfit": {"id": "sport", "vrm": sent_vrm, "fx": "none"}})]
    );
}
