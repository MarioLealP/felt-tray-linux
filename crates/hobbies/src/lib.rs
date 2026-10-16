//! WP-30: hobbies while Mario is AFK.
//!
//! On afk_start she starts a hobby, weighted by mood and hour, and moves from
//! one to the next every 5-20 min until he's back:
//!   - reading: sits down (the WP-21 chair), a book conjured into her hand;
//!   - gaming: the same with the controller;
//!   - napping: blanket + pillow laid out, sits down, eyes closed (late night
//!     or low energy make it likelier);
//!   - tidying: the WP-21 tidy routine, one piece of clutter at a time (only
//!     when there is clutter);
//!   - stretching: arm/neck stretches and looking around, a clip every ~minute.
//!
//! Each finished hobby is logged ("hobby" event -> the diary). On afk_end the
//! current hobby winds down (item dissolves, she stands up) and the list of
//! what she did rides along on the afk_end event, so felt-core's welcome-back
//! line can mention it.
//!
//! The item is conjured under the base lock and then held with the lock
//! RELEASED: a 20-minute read must not block every other animation (the
//! ambient loop skips while `active`, so nothing else moves her meanwhile).
//!
//! Missing for polish: real reading / doodling / lying-down clips (she reads
//! in the chair's seated idle), and a sketchbook prop for doodling.
//!
//! This port is pure logic: the Python module's asyncio task is an explicit
//! state machine the embedder drives with [`HobbyDirector::run_until_blocked`],
//! and cancelling the task (`HobbyDirector::stop`) runs the same `finally`
//! bookkeeping the Python cancellation does. Clock, randomness and every
//! `AppState` touch point are injected via [`Clock`], [`Rng`] and [`HobbyState`].

use std::collections::HashMap;

/// `felt_animation.presence.VISIBLE` - she is on the desktop.
pub const VISIBLE: &str = "visible";

/// Shortest hobby spell (seconds).
pub const MIN_S: f64 = 5.0 * 60.0;
/// Longest hobby spell (seconds).
pub const MAX_S: f64 = 20.0 * 60.0;
/// Standing breather between two hobbies: `(lo, hi)` seconds.
pub const GAP_S: (f64, f64) = (8.0, 20.0);

/// Stretch clips cycling while "stretching": a clip every ~minute.
pub const STRETCH_TAGS: &[&str] = &[
    "ambient/stretch_arms",
    "ambient/stretch_neck",
    "ambient/look_around",
    "ambient/weight_shift",
];

/// Hobbies that sit down with a prop in her hand: hobby -> item.
pub const SIT_ITEMS: &[(&str, &str)] = &[("reading", "book"), ("gaming", "controller")];

/// The nap face: eyes shut, a hint of sorrow (the `NAP_FACE` blendshapes).
pub fn nap_face() -> HashMap<String, f64> {
    HashMap::from([
        ("Fcl_EYE_Close".to_string(), 0.95),
        ("Fcl_BRW_Sorrow".to_string(), 0.1),
    ])
}

/// One finished hobby of this AFK stretch, as it rides along on the afk_end
/// event and into the diary ("hobby" event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HobbyEntry {
    pub hobby: String,
    pub minutes: i64,
}

/// Python's `round()` for the minutes bookkeeping: ties-to-even.
fn py_round(x: f64) -> i64 {
    let floor = x.floor();
    if x - floor == 0.5 {
        let f = floor as i64;
        // A tie lands on the even neighbour.
        if f % 2 == 0 { f } else { f + 1 }
    } else {
        x.round() as i64
    }
}

/// Pure: how likely each hobby is now.
pub fn hobby_weights(
    hour: i32,
    mood: &HashMap<String, f64>,
    can_tidy: bool,
    last: Option<&str>,
) -> HashMap<String, f64> {
    let energy = mood.get("energy").copied().unwrap_or(0.5);
    let boredom = mood.get("boredom").copied().unwrap_or(0.3);
    let late = !(6..23).contains(&hour);
    let mut w = HashMap::from([
        ("reading".to_string(), 3.0),
        ("gaming".to_string(), 2.0 + 2.0 * energy),
        (
            "napping".to_string(),
            0.5 + (if late { 3.0 } else { 0.0 }) + 3.0 * (0.5 - energy).max(0.0),
        ),
        (
            "tidying".to_string(),
            if can_tidy { 2.0 + 2.0 * boredom } else { 0.0 },
        ),
        ("stretching".to_string(), 1.5 + boredom),
    ]);
    if let Some(last) = last
        && let Some(weight) = w.get_mut(last)
    {
        *weight = 0.0; // never the same one twice in a row
    }
    w
}

/// The weights `pick` draws from (module-level `hobby_weights` by default;
/// tests replace it the way the Python tests monkeypatch it).
pub type WeightsFn = fn(i32, &HashMap<String, f64>, bool, Option<&str>) -> HashMap<String, f64>;

/// Weighted pick over the positive-weight entries; `None` when there are none.
pub fn pick(weights: &HashMap<String, f64>, rng: &mut impl Rng) -> Option<String> {
    let mut items: Vec<(&str, f64)> = weights
        .iter()
        .filter(|(_, v)| **v > 0.0)
        .map(|(k, v)| (k.as_str(), *v))
        .collect();
    if items.is_empty() {
        return None;
    }
    // A stable order keeps the draw deterministic for a given rng.
    items.sort_by(|a, b| a.0.cmp(b.0));
    let total: f64 = items.iter().map(|(_, v)| *v).sum();
    let mut x = rng.uniform(0.0, total);
    for (k, v) in items {
        x -= v;
        if x < 0.0 {
            return Some(k.to_string());
        }
    }
    // Only possible through float rounding at the very tail.
    weights
        .iter()
        .filter(|(_, v)| **v > 0.0)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, _)| k.clone())
}

/// The wall/monotonic clock. `monotonic` is `time.monotonic`, behind a name
/// tests can replace (patching the time module itself would also freeze
/// asyncio's clock); `hour` is `datetime.datetime.now().hour`.
pub trait Clock {
    fn monotonic(&self) -> f64;
    fn hour(&self) -> i32;
}

/// A clock tests (and embedders) drive by hand.
pub struct ManualClock {
    now: f64,
    hour: i32,
}

impl ManualClock {
    pub fn new(now: f64, hour: i32) -> Self {
        Self { now, hour }
    }
    pub fn set_now(&mut self, now: f64) {
        self.now = now;
    }
    pub fn set_hour(&mut self, hour: i32) {
        self.hour = hour;
    }
}

impl Clock for ManualClock {
    fn monotonic(&self) -> f64 {
        self.now
    }
    fn hour(&self) -> i32 {
        self.hour
    }
}

/// The slice of `random.Random` this module uses.
pub trait Rng {
    /// `random.uniform(lo, hi)`.
    fn uniform(&mut self, lo: f64, hi: f64) -> f64;
    /// `random.choice(seq)` - uniform index into `0..n`.
    fn index_below(&mut self, n: usize) -> usize;
}

/// Seedable fallback randomness for embedders who don't bring their own
/// (`random.Random()` in the Python).
pub struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        // SplitMix64.
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Rng for SimpleRng {
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * u
    }
    fn index_below(&mut self, n: usize) -> usize {
        assert!(n > 0, "index_below on an empty slice");
        (self.next_u64() % n as u64) as usize
    }
}

/// The slice of `AppState` (plus `event_log.report_event` and
/// `behavior.update_resting_face`) the hobby director touches. The real
/// adapters live outside this crate.
pub trait HobbyState {
    /// `state.presence.state` - compared against [`VISIBLE`].
    fn presence_state(&self) -> &str;
    /// `state.mood.as_dict()`.
    fn mood(&self) -> HashMap<String, f64>;
    /// Membership of `state.known_tags`.
    fn has_known_tag(&self, tag: &str) -> bool;
    /// `state.props_director.enabled`.
    fn props_enabled(&self) -> bool;
    /// `state.props_director.tidy_list()` - non-empty means clutter to tidy.
    fn tidy_list(&self) -> Vec<String>;
    /// `state.props_director.tidy_one()`.
    fn tidy_one(&mut self) -> bool;
    /// `state.props_director.sit_down()`.
    fn sit_down(&mut self) -> bool;
    /// `state.props_director.conjure_held(item, bone)`.
    fn conjure_held(&mut self, item: &str, bone: &str) -> String;
    /// `state.props_director.dismiss(pid)`.
    fn dismiss(&mut self, pid: &str);
    /// `state.props_director.stand_up_if_sitting()`.
    fn stand_up_if_sitting(&mut self);
    /// `state.props_director.use({"item": item, "place": place})`.
    fn use_item(&mut self, item: &str, place: bool) -> bool;
    /// Enter `state.behavior.hold_base()` (before the locked sequence).
    fn hold_base_enter(&mut self);
    /// Leave `state.behavior.hold_base()` (after the locked sequence).
    fn hold_base_exit(&mut self);
    /// `state.overlay.resting_face(shapes)`.
    fn resting_face(&mut self, shapes: &HashMap<String, f64>) -> bool;
    /// `behavior.run_sequence(state, tags)`.
    fn run_sequence(&mut self, tags: &[String]);
    /// `behavior.update_resting_face(state, force)`.
    fn update_resting_face(&mut self, force: bool);
    /// `event_log.report_event(state, kind, data)`.
    fn report_event(&mut self, kind: &str, data: &HobbyEntry);
}

/// Where the run loop is: its own `_run`/`_do` await points.
#[derive(Debug)]
enum Stage {
    /// Top of `_run`'s while loop (never a suspension point).
    Pick,
    /// Inside `_do`'s `asyncio.sleep(duration)` (sitting/nap hobby).
    Hobby,
    /// Between `tidy_one` calls, or at the top of the tidying while loop.
    Tidy { end: f64 },
    /// Between stretch clips, or at the top of the stretching while loop.
    Stretch { end: f64, tags: Vec<String> },
    /// `_run`'s `asyncio.sleep(rng.uniform(*GAP_S))` between hobbies.
    Gap,
}

/// WP-30's `HobbyDirector`. The Python runs `_run` as an asyncio task and
/// `stop()` cancels it; here the embedder pumps [`Self::run_until_blocked`]
/// and [`Self::stop`] plays the cancellation (`finally` bookkeeping included).
pub struct HobbyDirector<S: HobbyState, R: Rng, C: Clock> {
    pub state: S,
    pub rng: R,
    pub clock: C,
    min_s: f64,
    max_s: f64,
    weights_fn: WeightsFn,
    /// `task is not None and not task.done()`.
    running: bool,
    /// The coroutine body has begun (its `try` is live).
    began: bool,
    stage: Stage,
    /// Absolute `monotonic` deadline of the sleep we are blocked on.
    wait_until: Option<f64>,
    current: Option<String>,
    /// This AFK stretch: every finished hobby, `{"hobby", "minutes"}`.
    done: Vec<HobbyEntry>,
    /// Prop id in her hand.
    held: Option<String>,
    last: Option<String>,
    started: f64,
}

impl<S: HobbyState, R: Rng, C: Clock> HobbyDirector<S, R, C> {
    pub fn new(state: S, rng: R, clock: C) -> Self {
        Self {
            state,
            rng,
            clock,
            min_s: MIN_S,
            max_s: MAX_S,
            weights_fn: hobby_weights,
            running: false,
            began: false,
            stage: Stage::Pick,
            wait_until: None,
            current: None,
            done: Vec::new(),
            held: None,
            last: None,
            started: 0.0,
        }
    }

    /// `active` - a run is in flight.
    pub fn active(&self) -> bool {
        self.running
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn done(&self) -> &[HobbyEntry] {
        &self.done
    }

    pub fn clock_mut(&mut self) -> &mut C {
        &mut self.clock
    }

    pub fn state(&self) -> &S {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Tests replace `hobby_weights` with a one-hobby stub (the Python
    /// tests' `monkeypatch.setattr(hobbies, "hobby_weights", ...)`).
    pub fn set_weights_fn(&mut self, f: WeightsFn) {
        self.weights_fn = f;
    }

    /// Tests pin `MIN_S`/`MAX_S` so the spell length is a known value.
    pub fn set_duration_bounds(&mut self, min_s: f64, max_s: f64) {
        self.min_s = min_s;
        self.max_s = max_s;
    }

    /// Begin an AFK stretch. No-op while still active, while she is hidden,
    /// or while props are disabled.
    pub fn start(&mut self) {
        if self.active() || self.state.presence_state() != VISIBLE || !self.state.props_enabled() {
            return;
        }
        self.done.clear();
        self.running = true;
        self.began = false;
        self.stage = Stage::Pick;
        self.wait_until = None;
    }

    /// Wind down (cancel, clean up) and return what she did.
    pub fn stop(&mut self) -> Vec<HobbyEntry> {
        if self.running && self.began {
            // Cancelled inside `_do`: its `finally` still runs. Cancelled on
            // the gap sleep: `_do`'s `finally` already ran, so nothing here.
            if matches!(
                self.stage,
                Stage::Hobby | Stage::Tidy { .. } | Stage::Stretch { .. }
            ) {
                self.finish_hobby();
            }
        }
        self.running = false;
        self.began = false;
        self.wait_until = None;
        self.stage = Stage::Pick;
        self.current = None;
        self.done.clone()
    }

    /// Drive the run loop until it blocks on a sleep (returns that sleep's
    /// `monotonic` deadline) or the run ends (`None`). When the deadline has
    /// passed, the sleep finishes and the loop continues.
    pub fn run_until_blocked(&mut self) -> Option<f64> {
        if !self.running {
            return None;
        }
        self.began = true;
        loop {
            if let Some(t) = self.wait_until {
                if self.clock.monotonic() < t {
                    return Some(t);
                }
                // The sleep finished the normal way (not cancelled).
                self.wait_until = None;
                if matches!(self.stage, Stage::Hobby) {
                    self.finish_hobby();
                    self.schedule_gap();
                } else if matches!(self.stage, Stage::Gap) {
                    self.stage = Stage::Pick;
                }
                // Tidy/stretch: back at the top of the while loop.
            }
            match self.stage {
                Stage::Pick => {
                    if self.state.presence_state() != VISIBLE {
                        self.running = false;
                        self.current = None;
                        return None;
                    }
                    let hour = self.clock.hour();
                    let mood = self.state.mood();
                    let can_tidy = !self.state.tidy_list().is_empty();
                    let weights = (self.weights_fn)(hour, &mood, can_tidy, self.last.as_deref());
                    let Some(hobby) = pick(&weights, &mut self.rng) else {
                        self.running = false;
                        self.current = None;
                        return None;
                    };
                    let duration = self.rng.uniform(self.min_s, self.max_s);
                    self.current = Some(hobby.clone());
                    self.started = self.clock.monotonic();
                    self.begin_hobby(&hobby, duration);
                }
                Stage::Tidy { end } => {
                    if self.clock.monotonic() < end && !self.state.tidy_list().is_empty() {
                        self.state.tidy_one();
                        let gap = self.rng.uniform(10.0, 30.0);
                        self.wait_until = Some(self.clock.monotonic() + gap);
                    } else {
                        self.finish_hobby();
                        self.schedule_gap();
                    }
                }
                Stage::Stretch { end, ref tags } => {
                    // One clip, or the while loop is over.
                    let next = if self.clock.monotonic() < end && !tags.is_empty() {
                        let i = self.rng.index_below(tags.len());
                        Some(tags[i].clone())
                    } else {
                        None
                    };
                    match next {
                        Some(tag) => {
                            self.state.run_sequence(&[tag]);
                            let gap = self.rng.uniform(40.0, 90.0);
                            self.wait_until = Some(self.clock.monotonic() + gap);
                        }
                        None => {
                            self.finish_hobby();
                            self.schedule_gap();
                        }
                    }
                }
                // A sleep is always pending in these stages.
                Stage::Hobby | Stage::Gap => return self.wait_until,
            }
        }
    }

    /// `_do`: the hobby's immediate setup, then its sleep(s).
    fn begin_hobby(&mut self, hobby: &str, duration: f64) {
        let item = SIT_ITEMS
            .iter()
            .find_map(|(h, i)| (*h == hobby).then_some(*i));
        if item.is_some() || hobby == "napping" {
            if hobby == "napping" {
                self.state.use_item("blanket", true);
                self.state.use_item("pillow", true);
            }
            if !self.state.sit_down() {
                self.finish_hobby();
                self.schedule_gap();
                return;
            }
            match item {
                Some(item) => {
                    self.state.hold_base_enter();
                    let pid = self.state.conjure_held(item, "RightHand");
                    self.held = Some(pid);
                    self.state.hold_base_exit();
                }
                None => {
                    self.state.resting_face(&nap_face());
                }
            }
            self.wait_until = Some(self.clock.monotonic() + duration);
            self.stage = Stage::Hobby;
        } else if hobby == "tidying" {
            let end = self.clock.monotonic() + duration;
            self.stage = Stage::Tidy { end };
        } else if hobby == "stretching" {
            let end = self.clock.monotonic() + duration;
            let tags: Vec<String> = STRETCH_TAGS
                .iter()
                .copied()
                .filter(|t| self.state.has_known_tag(t))
                .map(|t| t.to_string())
                .collect();
            self.stage = Stage::Stretch { end, tags };
        } else {
            self.finish_hobby();
            self.schedule_gap();
        }
    }

    /// `_do`'s `finally`: minutes rounded, cleanup, diary entry. Never raises.
    fn finish_hobby(&mut self) {
        let Some(hobby) = self.current.take() else {
            return;
        };
        let minutes = py_round((self.clock.monotonic() - self.started) / 60.0);
        self.cleanup();
        self.last = Some(hobby.clone());
        if minutes >= 1 {
            let entry = HobbyEntry { hobby, minutes };
            self.done.push(entry.clone());
            self.state.report_event("hobby", &entry);
        }
    }

    /// `_cleanup`: item dissolves, face back to mood, stand up. Never raises.
    fn cleanup(&mut self) {
        self.state.hold_base_enter();
        if let Some(pid) = self.held.take() {
            self.state.dismiss(&pid);
        }
        self.state.stand_up_if_sitting();
        self.state.hold_base_exit();
        self.state.update_resting_face(true);
    }

    /// `_run`'s standing breather between two hobbies.
    fn schedule_gap(&mut self) {
        let gap = self.rng.uniform(GAP_S.0, GAP_S.1);
        self.wait_until = Some(self.clock.monotonic() + gap);
        self.stage = Stage::Gap;
    }
}
