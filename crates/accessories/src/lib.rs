//! WP-32: accessories she wears - by outfit, and by mood/ambient.
//!
//! Worn items are WP-21 props held on a bone (hats on Head, bags on Chest/Hips),
//! kept for minutes to hours, not a quick prop episode. One item per slot
//! (props.json "accessories"."slots", Mario 2026-10-01: "she can combine the
//! backpack with the hats, but not the backpack with the side bag, or the santa
//! hat with the witch hat"):
//!   hat (hats + ear headbands) | hair | face (glasses) | neck | bag
//!
//! Two sources:
//!   - outfit: outfits.json "accessories": [...] - on while that outfit is on
//!     (set after every {"outfit_applied"}); always wins its slot;
//!   - ambient: every AMBIENT_EVERY_S, maybe one more in a free slot, picked by
//!     mood / weather / hour from each item's tags; worn 20-90 min.
//!
//! A restarted overlay forgets held props, so on reconnect everything is
//! cleared and the outfit's set comes back with the outfit (on_connected).
//!
//! The Python module awaits the prop overlay; this crate holds the pure
//! decision logic and reaches the outside only through the [`AccessoryState`],
//! [`Clock`] and [`Rng`] traits, so tests can inject fakes and stay
//! deterministic.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Value, json};

pub const AMBIENT_EVERY_S: f64 = 180.0;
pub const AMBIENT_CHANCE: f64 = 0.12; // per check, when a slot is free and nothing else is going on
pub const AMBIENT_MIN_S: f64 = 20.0 * 60.0;
pub const AMBIENT_MAX_S: f64 = 90.0 * 60.0;

// -- shared stand-ins for the felt_animation collaborators ---------------------
// (runtime.AppState, props.PropDirector, mood, event_log.report_event)

/// The slice of `runtime.AppState` the director touches, plus the
/// `event_log.report_event` telemetry hook. The Python wraps the prop calls
/// in `behavior.hold_base()` (the base lock, state.py); implementations here
/// hold that lock inside `conjure_held` / `dismiss` themselves.
pub trait AccessoryState {
    /// What the prop calls fail with (`props.PropDirector` errors).
    type Error;

    /// `props_director.enabled` - false: props are off, `wear` is a no-op.
    fn props_enabled(&self) -> bool;

    /// `props_director.manifest` - the props.json tree ("items",
    /// "accessories" with "slots"/"tags", ...).
    fn manifest(&self) -> &Value;

    /// `mood.as_dict()` - mood dimensions, e.g. "happiness" -> 0.9.
    fn mood(&self) -> &HashMap<String, f64>;

    /// `state.weather_condition` - e.g. Some("sun"); usually None.
    fn weather_condition(&self) -> Option<&str>;

    /// `props_director.conjure_held(item, bone)` - spawn the prop held on
    /// `bone` (base lock held), return its pid.
    fn conjure_held(&mut self, item: &str, bone: &str) -> Result<String, Self::Error>;

    /// `props_director.dismiss(pid)` - dissolve the prop out (base lock held).
    fn dismiss(&mut self, pid: &str) -> Result<(), Self::Error>;

    /// `event_log.report_event(state, kind, data)` - fire-and-forget diary
    /// telemetry; must never raise past itself.
    fn report_event(&mut self, kind: &str, data: &Value);
}

/// Injectable time: the Python module's `_now()` (`time.monotonic`) and the
/// local wall-clock hour (`datetime.datetime.now().hour`) that `tick` picks
/// ambient pieces with.
pub trait Clock {
    /// Monotonic seconds (`time.monotonic()`); only differences matter.
    fn monotonic(&self) -> f64;

    /// Local hour of day, 0..=23.
    fn hour(&self) -> i32;
}

/// Injectable randomness: the `random.Random` the director is built with.
pub trait Rng {
    /// `random.Random.random()` - a float in `[0.0, 1.0)`.
    fn random(&mut self) -> f64;

    /// `random.Random.choice(items)` - uniformly one of `items`, or None if
    /// it is empty (the Python raises IndexError there; callers guard).
    fn choose<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        // `random()` scaled to a bucket; only used to spread choices.
        let n = items.len();
        let i = (self.random() * n as f64) as usize;
        Some(&items[i.min(n - 1)])
    }

    /// `random.Random.uniform(lo, hi)` for floats: `lo + (hi - lo) * random()`.
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.random()
    }
}

/// Deterministic SplitMix64 `Rng`, standing in for Python's `random.Random(seed)`
/// when a reproducible stream is wanted (tests, replay).
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Rng for SeededRng {
    fn random(&mut self) -> f64 {
        // top 53 bits -> [0.0, 1.0)
        (self.next_u64() >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
    }
}

// -- worn pieces ----------------------------------------------------------------

/// One worn piece: the Python `worn[slot]` dict.
#[derive(Debug, Clone, PartialEq)]
pub struct WornItem {
    pub item: String,
    pub pid: String,
    /// "outfit" or "ambient".
    pub source: String,
    /// Monotonic deadline; None = until the outfit changes or the overlay restarts.
    pub until: Option<f64>,
}

/// `snapshot()` value: the public face of a worn piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WornSummary {
    pub item: String,
    pub source: String,
}

/// Pure: which item tags fit right now.
pub fn context_tags(
    hour: i32,
    mood: &HashMap<String, f64>,
    weather: Option<&str>,
) -> BTreeSet<String> {
    let mut tags = BTreeSet::from(["casual".to_string()]);
    if mood.get("happiness").copied().unwrap_or(0.5) >= 0.6
        || mood.get("affection").copied().unwrap_or(0.5) >= 0.6
    {
        tags.insert("cute".to_string());
    }
    if matches!(weather, Some("sun" | "clear")) && (9..19).contains(&hour) {
        tags.insert("summer".to_string());
    }
    if !(2..18).contains(&hour) {
        tags.insert("evening".to_string());
    }
    tags
}

/// Wears one item per slot from two sources (outfit, ambient).
pub struct AccessoryDirector<S, R, C> {
    pub state: S,
    pub rng: R,
    pub clock: C,
    /// slot -> worn piece (Python: `self.worn`).
    pub worn: HashMap<String, WornItem>,
    /// Monotonic time of the last ambient check (Python: `self._last_check`).
    pub last_check: f64,
}

impl<S: AccessoryState, R: Rng, C: Clock> AccessoryDirector<S, R, C> {
    pub fn new(state: S, rng: R, clock: C) -> Self {
        Self {
            state,
            rng,
            clock,
            worn: HashMap::new(),
            last_check: 0.0,
        }
    }

    // -- catalogue -------------------------------------------------------

    /// Which slot ("hat", "bag", ...) this item occupies: its first match in
    /// the manifest's `accessories`.`slots`, or None.
    pub fn slot_of(&self, item: &str) -> Option<String> {
        let slots = self
            .state
            .manifest()
            .get("accessories")
            .and_then(|a| a.get("slots"))
            .and_then(Value::as_object)?;
        for (slot, items) in slots {
            if items
                .as_array()
                .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(item)))
            {
                return Some(slot.clone());
            }
        }
        None
    }

    /// The bone the item attaches to - the first key of its `grip` dict
    /// (hats on "Head", bags on "Chest"/"Hips").
    fn bone_of(&self, item: &str) -> Option<String> {
        let grip = self
            .state
            .manifest()
            .get("items")
            .and_then(|items| items.get(item))
            .and_then(|entry| entry.get("grip"))
            .and_then(Value::as_object); // Python: `.get("grip") or {}`
        grip.and_then(|grip| grip.keys().next()).cloned()
    }

    // -- wearing ---------------------------------------------------------

    /// Put `item` on from `source` ("outfit" | "ambient"; `seconds` is how
    /// long an ambient piece is worn). False when the item has no slot or
    /// bone, or props are off. The outfit's piece keeps its slot against
    /// anything but another outfit's piece.
    pub fn wear(
        &mut self,
        item: &str,
        source: &str,
        seconds: Option<f64>,
    ) -> Result<bool, S::Error> {
        let (Some(slot), Some(bone)) = (self.slot_of(item), self.bone_of(item)) else {
            return Ok(false);
        };
        if !self.state.props_enabled() {
            return Ok(false);
        }
        if let Some(cur) = self.worn.get_mut(&slot) {
            if cur.item == item {
                // the same piece again: "outfit" on either side upgrades it
                // to outfit wear (no deadline); otherwise the deadline stands.
                if source == "outfit" || cur.source == "outfit" {
                    cur.source = "outfit".to_string();
                    cur.until = None;
                } else {
                    cur.source = source.to_string();
                }
                return Ok(true);
            }
            if cur.source == "outfit" && source != "outfit" {
                return Ok(false); // the outfit's piece keeps its slot
            }
        }
        self.take_off(&slot);
        let pid = self.state.conjure_held(item, &bone)?;
        let until = seconds.map(|s| self.clock.monotonic() + s);
        self.worn.insert(
            slot,
            WornItem {
                item: item.to_string(),
                pid,
                source: source.to_string(),
                until,
            },
        );
        self.state
            .report_event("accessory", &json!({"item": item, "source": source}));
        Ok(true)
    }

    /// Take the slot's piece off. A dismiss failure is swallowed (the Python
    /// logs it): the worn record is gone either way, and this never fails.
    pub fn take_off(&mut self, slot: &str) {
        let Some(cur) = self.worn.remove(slot) else {
            return;
        };
        let _ = self.state.dismiss(&cur.pid);
    }

    /// After an outfit change: its accessories on, the previous outfit's off.
    pub fn set_outfit(&mut self, items: &[String]) -> Result<(), S::Error> {
        // slot -> item; a later item for the same slot wins in place (Python dict).
        let mut wanted: Vec<(String, String)> = Vec::new();
        for i in items {
            if let Some(slot) = self.slot_of(i) {
                match wanted.iter().position(|e| e.0 == slot) {
                    Some(idx) => wanted[idx].1 = i.clone(),
                    None => wanted.push((slot, i.clone())),
                }
            }
        }
        let mut stale: Vec<String> = Vec::new();
        for (slot, cur) in self.worn.iter() {
            if cur.source == "outfit" && !wanted.iter().any(|(s, i)| s == slot && i == &cur.item) {
                stale.push(slot.clone());
            }
        }
        for slot in stale {
            self.take_off(&slot);
        }
        for (_, item) in &wanted {
            self.wear(item, "outfit", None)?;
        }
        Ok(())
    }

    /// The overlay restarted: whatever she held is gone with it.
    pub fn reset(&mut self) {
        self.worn.clear();
    }

    // -- ambient ---------------------------------------------------------

    /// From the ambient loop: expire ambient pieces, maybe put one on.
    pub fn tick(&mut self) -> Result<(), S::Error> {
        let now = self.clock.monotonic();
        let mut expired: Vec<String> = Vec::new();
        for (slot, cur) in self.worn.iter() {
            if cur.until.is_some_and(|until| now >= until) {
                expired.push(slot.clone());
            }
        }
        for slot in expired {
            self.take_off(&slot);
        }
        if now - self.last_check < AMBIENT_EVERY_S {
            return Ok(());
        }
        self.last_check = now;
        if self.rng.random() >= AMBIENT_CHANCE {
            return Ok(());
        }
        let hour = self.clock.hour();
        if let Some(item) = self.pick(hour).filter(|item| !item.is_empty()) {
            let seconds = self.rng.uniform(AMBIENT_MIN_S, AMBIENT_MAX_S);
            self.wear(&item, "ambient", Some(seconds))?;
        }
        Ok(())
    }

    /// Pick an ambient piece for `hour`: an item in a free slot whose tags fit
    /// the mood / weather / hour context, or None when nothing does.
    pub fn pick(&mut self, hour: i32) -> Option<String> {
        let tags = context_tags(hour, self.state.mood(), self.state.weather_condition());
        let accessories = self.state.manifest().get("accessories");
        let item_tags = accessories
            .and_then(|a| a.get("tags"))
            .and_then(Value::as_object);
        let slots = accessories
            .and_then(|a| a.get("slots"))
            .and_then(Value::as_object);
        let mut options: Vec<String> = Vec::new();
        if let Some(slots) = slots {
            for (slot, items) in slots {
                if self.worn.contains_key(slot.as_str()) {
                    continue;
                }
                for item in items
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let tagged = item_tags
                        .and_then(|t| t.get(item))
                        .and_then(Value::as_array)
                        .is_some_and(|list| {
                            list.iter()
                                .filter_map(Value::as_str)
                                .any(|t| tags.contains(t))
                        });
                    if tagged {
                        options.push(item.to_string());
                    }
                }
            }
        }
        self.rng.choose(&options).cloned()
    }

    /// What she is wearing, for the overlay/status: slot -> {item, source}.
    pub fn snapshot(&self) -> HashMap<String, WornSummary> {
        self.worn
            .iter()
            .map(|(slot, c)| {
                (
                    slot.clone(),
                    WornSummary {
                        item: c.item.clone(),
                        source: c.source.clone(),
                    },
                )
            })
            .collect()
    }
}
