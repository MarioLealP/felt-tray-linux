//! [`OutfitDirector`]: the persisted state and the change choreography.
//!
//! The Python original drives the choreography with asyncio tasks; this port
//! runs each one on a std thread and serialises on the same flags/waits
//! (the swap message's `outfit_applied` reply is the rendezvous point).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::datetime::DateTime;
use crate::rng::OutfitRng;
use crate::state::{HIDDEN, OutfitState, VISIBLE};
use crate::wardrobe::WardrobeStore;
use crate::{
    APPLY_TIMEOUT_S, DEFAULT_ID, FREE_WAIT_S, OVERRIDE_SECONDS, Outfit, PRESENCE_WAIT_S,
    QUICK_SWAP_DELAY_S, SLEEPY_ENERGY, ambient_pick, bikini_roll, effective_date, pick_daily,
    pool_ids, scheduled_outfit,
};

/// Injected time and randomness (Python takes these as constructor args).
pub struct DirectorHooks {
    pub rng: Box<dyn OutfitRng>,
    pub clock: Box<dyn Fn() -> DateTime + Send + Sync>,
    pub mono: Box<dyn Fn() -> f64 + Send + Sync>,
}

/// Tunables; defaults match the Python constructor.
#[derive(Debug, Clone, Copy)]
pub struct DirectorConfig {
    pub ambient_chance: f64,
    pub bikini_chance: f64,
    pub clip_timeout: f64,
    /// Test seam for the `QUICK_SWAP_DELAY_S` module constant (the Python
    /// tests monkeypatch it to 0).
    pub quick_swap_delay_s: f64,
}

impl Default for DirectorConfig {
    fn default() -> Self {
        Self {
            ambient_chance: 0.0,
            bikini_chance: 0.01,
            clip_timeout: 30.0,
            quick_swap_delay_s: QUICK_SWAP_DELAY_S,
        }
    }
}

/// One spawned choreography task (Python's `asyncio.Task` handle).
pub struct TaskSlot {
    done: AtomicBool,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
}

/// The spawn registry entries (`(task id, task)`), Python's `d._tasks` items.
type TaskEntry = (usize, Arc<TaskSlot>);

impl TaskSlot {
    fn new() -> Self {
        Self {
            done: AtomicBool::new(false),
            handle: Mutex::new(None),
        }
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Await the task. A panicking task propagates, like `await task`.
    pub fn join(&self) {
        let handle = self.handle.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().expect("outfits: spawned task panicked");
        }
    }
}

/// Resets `changing` however the swap leaves (Python's try/finally).
struct ChangingGuard<'a>(&'a AtomicBool);

impl Drop for ChangingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct OutfitDirector {
    state: Arc<OutfitState>,
    outfits: Vec<Outfit>,
    by_id: HashMap<String, Outfit>,
    ambient_chance: f64,
    bikini_chance: Mutex<f64>,
    clip_timeout: f64,
    quick_swap_delay_s: Mutex<f64>,
    rng: Mutex<Box<dyn OutfitRng>>,
    clock: Box<dyn Fn() -> DateTime + Send + Sync>,
    mono: Box<dyn Fn() -> f64 + Send + Sync>,
    wardrobe: Arc<dyn WardrobeStore>,
    changing: AtomicBool,
    current: Mutex<String>,
    current_vrm: Mutex<Option<String>>,
    override_: Mutex<Option<Value>>,
    waiters: Mutex<HashMap<String, mpsc::Sender<bool>>>,
    tasks: Arc<Mutex<Vec<TaskEntry>>>,
    next_task_id: AtomicUsize,
}

impl OutfitDirector {
    pub fn new(
        state: Arc<OutfitState>,
        outfits: Vec<Outfit>,
        wardrobe: Arc<dyn WardrobeStore>,
        hooks: DirectorHooks,
        config: DirectorConfig,
    ) -> Self {
        let by_id: HashMap<String, Outfit> =
            outfits.iter().map(|e| (e.id.clone(), e.clone())).collect();
        let ws = wardrobe.load();
        let current = match ws.get("outfit_current").and_then(Value::as_str) {
            Some(cur) if by_id.contains_key(cur) => cur.to_string(),
            _ => DEFAULT_ID.to_string(),
        };
        // Which VRM of the current outfit she's wearing: the original or one
        // of its Tier-A variants (outfits.json "variants", Mario's keepers).
        let current_vrm = ws
            .get("outfit_current_vrm")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|v| {
                by_id
                    .get(&current)
                    .is_some_and(|e| e.variants.iter().any(|var| var == v) || e.vrm == *v)
            });
        let override_ = match ws.get("outfit_override") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.clone()),
        };
        Self {
            state,
            outfits,
            by_id,
            ambient_chance: config.ambient_chance,
            bikini_chance: Mutex::new(config.bikini_chance),
            clip_timeout: config.clip_timeout,
            quick_swap_delay_s: Mutex::new(config.quick_swap_delay_s),
            rng: Mutex::new(hooks.rng),
            clock: hooks.clock,
            mono: hooks.mono,
            wardrobe,
            changing: AtomicBool::new(false),
            current: Mutex::new(current),
            current_vrm: Mutex::new(current_vrm),
            override_: Mutex::new(override_),
            waiters: Mutex::new(HashMap::new()),
            tasks: Arc::new(Mutex::new(Vec::new())),
            next_task_id: AtomicUsize::new(0),
        }
    }

    // -- persistence -------------------------------------------------------

    fn save_fields(&self, fields: &[(&str, Value)]) {
        let mut ws = self.wardrobe.load();
        for (key, value) in fields {
            ws.insert((*key).to_string(), value.clone());
        }
        self.wardrobe.save(&ws);
    }

    /// `_set_current` - also persists.
    pub fn set_current(&self, outfit_id: &str) {
        *self.current.lock().unwrap() = outfit_id.to_string();
        self.save_fields(&[("outfit_current", json!(outfit_id))]);
    }

    /// `_set_override` - also persists.
    pub fn set_override(&self, override_: Option<Value>) {
        *self.override_.lock().unwrap() = override_.clone();
        self.save_fields(&[("outfit_override", override_.unwrap_or(Value::Null))]);
    }

    /// Direct field poke (the Python tests assign `d.override = ...`).
    pub fn set_override_raw(&self, override_: Option<Value>) {
        *self.override_.lock().unwrap() = override_;
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn current_vrm(&self) -> Option<String> {
        self.current_vrm.lock().unwrap().clone()
    }

    pub fn override_value(&self) -> Option<Value> {
        self.override_.lock().unwrap().clone()
    }

    /// Test seam: `d.rng = random.Random(seed)`.
    pub fn set_rng(&self, rng: Box<dyn OutfitRng>) {
        *self.rng.lock().unwrap() = rng;
    }

    /// Test seam: `d.bikini_chance = 1.0`.
    pub fn set_bikini_chance(&self, chance: f64) {
        *self.bikini_chance.lock().unwrap() = chance;
    }

    /// Test seam: monkeypatching `QUICK_SWAP_DELAY_S`.
    pub fn set_quick_swap_delay(&self, seconds: f64) {
        *self.quick_swap_delay_s.lock().unwrap() = seconds;
    }

    // -- decisions ---------------------------------------------------------

    /// The feature is on when `default` is in the catalogue.
    pub fn enabled(&self) -> bool {
        self.by_id.contains_key(DEFAULT_ID)
    }

    fn sleepy(&self) -> bool {
        self.state.mood.energy().unwrap_or(1.0) < SLEEPY_ENERGY
    }

    /// Once per wardrobe day (first check after 05:00), never repeating
    /// yesterday's; persisted.
    pub fn daily_pick(&self, now: &DateTime) -> String {
        let ws = self.wardrobe.load();
        let today = effective_date(now).isoformat();
        if ws.get("outfit_daily_date").and_then(Value::as_str) == Some(today.as_str())
            && let Some(saved) = ws.get("outfit_daily").and_then(Value::as_str)
            && self.by_id.contains_key(saved)
        {
            return saved.to_string();
        }
        let prev = ws.get("outfit_daily").cloned();
        let yesterday = prev.as_ref().and_then(Value::as_str);
        let daily = pool_ids(&self.outfits, "daily");
        let chosen = {
            let mut rng = self.rng.lock().unwrap();
            pick_daily(&daily, yesterday, &mut *rng)
        };
        self.save_fields(&[
            ("outfit_daily", Value::String(chosen.clone())),
            ("outfit_daily_date", Value::String(today)),
            ("outfit_daily_prev", prev.unwrap_or(Value::Null)),
        ]);
        chosen
    }

    fn day_outfit(&self, now: &DateTime) -> String {
        let ov = self.override_value().filter(|ov| {
            ov.as_object().is_some_and(|ov| {
                let id_ok = ov
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| self.by_id.contains_key(id));
                let until = ov.get("until").and_then(Value::as_f64).unwrap_or(0.0);
                id_ok && until > now.timestamp()
            })
        });
        if let Some(ov) = ov
            && let Some(id) = ov.get("id").and_then(Value::as_str)
        {
            return id.to_string();
        }
        self.daily_pick(now)
    }

    pub fn target(&self, now: Option<&DateTime>) -> String {
        let owned;
        let now = match now {
            Some(now) => now,
            None => {
                owned = (self.clock)();
                &owned
            }
        };
        if let Some(sched) = scheduled_outfit(now, self.sleepy())
            && self.by_id.contains_key(sched)
        {
            // a scheduled change ends the override
            if self.override_value().is_some() {
                self.set_override(None);
            }
            return sched.to_string();
        }
        self.day_outfit(now)
    }

    /// Visible, idle, not seated, not speaking - else defer.
    pub fn can_change(&self) -> bool {
        !self.changing.load(Ordering::SeqCst)
            && self.state.presence.state() == VISIBLE
            && !self.state.behavior.busy()
            && self.state.behavior.posture() != "sitting"
            && self.state.seated_on_ledge().is_none()
            && self.state.props_director.seated_on().is_none()
            && !self.state.player.is_playing()
    }

    // -- triggers ----------------------------------------------------------

    /// Called from the ambient loop. Returns the outfit changed to.
    /// Blocking: the change choreography runs to completion on this thread.
    pub fn tick(&self, now: Option<&DateTime>) -> Option<String> {
        if !self.enabled() || !self.can_change() {
            return None;
        }
        let owned;
        let now = match now {
            Some(now) => now,
            None => {
                owned = (self.clock)();
                &owned
            }
        };
        let mut style = "proper";
        if scheduled_outfit(now, self.sleepy()).is_none() {
            let mut rng = self.rng.lock().unwrap();
            if rng.random() < self.ambient_chance {
                let daily = pool_ids(&self.outfits, "daily");
                let themed = pool_ids(&self.outfits, "themed");
                let pick = ambient_pick(&daily, &themed, &mut *rng);
                drop(rng);
                if pick != self.current() {
                    self.set_override(Some(json!({
                        "id": pick,
                        "until": now.timestamp() + OVERRIDE_SECONDS,
                    })));
                    style = "quick";
                }
            }
        }
        let want = self.target(Some(now));
        if want == self.current() {
            return None;
        }
        self.change_to(&want, style);
        Some(want)
    }

    /// A reply's animation tag arrived: a sunny weather answer rolls the
    /// 1% bikini. Deferred (a task) until she's free to change.
    /// Takes the `Arc` so the deferred task can outlive the call
    /// (`self: &Arc<Self>` is not a stable self type).
    pub fn on_reply_tag(self: Arc<Self>, tag: Option<&str>) {
        if !self.enabled() || !self.by_id.contains_key("bikini") {
            return;
        }
        let roll = {
            let mut rng = self.rng.lock().unwrap();
            bikini_roll(tag, &mut *rng, *self.bikini_chance.lock().unwrap())
        };
        if !roll {
            return;
        }
        // a date/night outfit outranks the bikini
        if scheduled_outfit(&(self.clock)(), self.sleepy()).is_some() {
            return;
        }
        let this = Arc::clone(&self);
        self.spawn(move || this.bikini_when_free());
    }

    fn bikini_when_free(&self) {
        let deadline = (self.mono)() + FREE_WAIT_S;
        while !self.can_change() {
            if (self.mono)() > deadline {
                return;
            }
            thread::sleep(Duration::from_secs_f64(1.0));
        }
        let now = (self.clock)();
        self.set_override(Some(json!({
            "id": "bikini",
            "until": now.timestamp() + OVERRIDE_SECONDS,
        })));
        self.change_to("bikini", "quick");
    }

    // -- choreography ------------------------------------------------------

    /// The outfit's original VRM plus its approved variants.
    pub fn vrms(&self, outfit_id: &str) -> Vec<String> {
        let Some(e) = self.by_id.get(outfit_id) else {
            return Vec::new();
        };
        let mut vrms = vec![e.vrm.clone()];
        vrms.extend(e.variants.iter().cloned());
        vrms
    }

    fn message(&self, outfit_id: &str, fx: &str, vrm: Option<String>) -> Value {
        let vrm = match vrm {
            Some(v) if !v.is_empty() => v,
            _ => self.by_id[outfit_id].vrm.clone(),
        };
        json!({"outfit": {"id": outfit_id, "vrm": vrm, "fx": fx}})
    }

    fn spin_tag(&self) -> Option<String> {
        let tags = self.state.known_tags.lock().unwrap();
        let mut matched: Vec<String> = tags
            .iter()
            .filter(|t| {
                let lowered = t.to_lowercase();
                lowered.contains("spin") || lowered.contains("twirl")
            })
            .cloned()
            .collect();
        matched.sort();
        if matched.is_empty() {
            return None;
        }
        let mut rng = self.rng.lock().unwrap();
        Some(matched[rng.choice_index(matched.len())].clone())
    }

    /// Serialised change. Returns true once the overlay accepted it.
    pub fn change_to(&self, outfit_id: &str, style: &str) -> bool {
        if !self.by_id.contains_key(outfit_id) {
            return false;
        }
        let expected = false;
        if self
            .changing
            .compare_exchange(expected, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        let _guard = ChangingGuard(&self.changing);
        if style == "proper" {
            self.proper(outfit_id)
        } else {
            self.quick(outfit_id)
        }
    }

    /// Send the swap; true if the overlay took it (and did not fail).
    fn send_and_wait(&self, outfit_id: &str, fx: &str) -> bool {
        let (tx, rx) = mpsc::channel();
        self.waiters
            .lock()
            .unwrap()
            .insert(outfit_id.to_string(), tx);
        // A change picks the original or one of its variants at random.
        let vrm = {
            let vrms = self.vrms(outfit_id);
            let mut rng = self.rng.lock().unwrap();
            vrms[rng.choice_index(vrms.len())].clone()
        };
        let result = (|| {
            if !self
                .state
                .overlay
                .send(&self.message(outfit_id, fx, Some(vrm.clone())))
            {
                return false;
            }
            self.set_current(outfit_id);
            *self.current_vrm.lock().unwrap() = Some(vrm.clone());
            self.save_fields(&[("outfit_current_vrm", json!(vrm))]);
            // no outfit_applied in time - the swap stands
            rx.recv_timeout(Duration::from_secs_f64(APPLY_TIMEOUT_S))
                .unwrap_or(true)
        })();
        self.waiters.lock().unwrap().remove(outfit_id);
        result
    }

    fn quick(&self, outfit_id: &str) -> bool {
        let Some(tag) = self.spin_tag() else {
            let _hold = self.state.behavior.hold_base();
            return self.send_and_wait(outfit_id, "puff");
        };
        let behavior = Arc::clone(&self.state.behavior);
        let clip_timeout = self.clip_timeout;
        let clip = thread::spawn(move || behavior.run_sequence(vec![tag], clip_timeout));
        thread::sleep(Duration::from_secs_f64(
            *self.quick_swap_delay_s.lock().unwrap(),
        ));
        let result = self.send_and_wait(outfit_id, "puff");
        // a dead clip must not sink the swap
        let _ = clip.join();
        result
    }

    fn proper(&self, outfit_id: &str) -> bool {
        self.state.presence_router.do_hide();
        if !self.wait_presence(HIDDEN) {
            if self.state.presence.state() != VISIBLE {
                self.state.presence_router.do_show();
            }
            return false;
        }
        let ok = self.send_and_wait(outfit_id, "none");
        self.state.presence_router.do_show();
        self.wait_presence(VISIBLE);
        ok
    }

    fn wait_presence(&self, want: &str) -> bool {
        let deadline = (self.mono)() + PRESENCE_WAIT_S;
        while self.state.presence.state() != want {
            if (self.mono)() > deadline {
                return false;
            }
            thread::sleep(Duration::from_secs_f64(0.25));
        }
        true
    }

    // -- overlay hooks -----------------------------------------------------

    /// A restarted overlay wears felt.vrm - put the current outfit back.
    pub fn on_connected(&self) {
        if let Some(accessories) = &self.state.accessories {
            accessories.reset(); // a restarted overlay holds nothing
        }
        if !self.enabled() || self.current() == DEFAULT_ID {
            return;
        }
        let vrm = self.current_vrm();
        self.state
            .overlay
            .send(&self.message(&self.current(), "none", vrm));
    }

    pub fn on_overlay_message(&self, msg: &Value) {
        if let Some(applied) = msg.get("outfit_applied").and_then(Value::as_object) {
            let oid = msg_id(applied.get("id"));
            self.resolve(&oid, true);
            if oid == DEFAULT_ID {
                self.spawn(self.reapply_tee_job());
            }
            // WP-32: this outfit's accessories on, the previous outfit's off.
            if let Some(accessories) = &self.state.accessories {
                let wanted = self
                    .by_id
                    .get(&oid)
                    .map(|e| e.accessories.clone())
                    .unwrap_or_default();
                let accessories = Arc::clone(accessories);
                self.spawn(move || accessories.set_outfit(wanted));
            }
        }
        if let Some(failed) = msg.get("outfit_failed").and_then(Value::as_object) {
            let oid = msg_id(failed.get("id"));
            self.resolve(&oid, false);
            if oid != DEFAULT_ID && self.by_id.contains_key(DEFAULT_ID) {
                self.set_current(DEFAULT_ID);
                self.set_override(None);
                let overlay = Arc::clone(&self.state.overlay);
                let fallback = self.message(DEFAULT_ID, "none", None);
                self.spawn(move || {
                    overlay.send(&fallback);
                });
            }
        }
    }

    fn resolve(&self, outfit_id: &str, ok: bool) {
        if let Some(tx) = self.waiters.lock().unwrap().get(outfit_id) {
            let _ = tx.send(ok);
        }
    }

    /// `_spawn` - run one choreography job in the background.
    pub fn spawn<F>(&self, job: F) -> Arc<TaskSlot>
    where
        F: FnOnce() + Send + 'static,
    {
        let slot = Arc::new(TaskSlot::new());
        let id = self.next_task_id.fetch_add(1, Ordering::SeqCst);
        self.tasks.lock().unwrap().push((id, Arc::clone(&slot)));
        let tasks = Arc::clone(&self.tasks);
        let slot_in_thread = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            job();
            slot_in_thread.done.store(true, Ordering::SeqCst);
            tasks.lock().unwrap().retain(|(tid, _)| *tid != id);
        });
        *slot.handle.lock().unwrap() = Some(handle);
        slot
    }

    /// The spawn registry, for tests (`d._tasks`).
    pub fn task_count(&self) -> usize {
        self.tasks.lock().unwrap().len()
    }

    /// `next(iter(d._tasks))` for tests.
    pub fn first_task(&self) -> Option<Arc<TaskSlot>> {
        self.tasks
            .lock()
            .unwrap()
            .first()
            .map(|(_, slot)| Arc::clone(slot))
    }

    fn reapply_tee_job(&self) -> impl FnOnce() + Send + 'static {
        let wardrobe = Arc::clone(&self.wardrobe);
        let overlay = Arc::clone(&self.state.overlay);
        move || {
            let tee = wardrobe
                .load()
                .get("current_tee")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(tee) = tee
                && !tee.is_empty()
            {
                wardrobe.apply_tee(overlay.as_ref(), &tee);
            }
        }
    }

    /// Debug: wear `outfit_id` now (held as a 2 h override so the next
    /// tick doesn't undo it, unless a date/night rule outranks it).
    pub fn force(&self, outfit_id: &str, proper: bool) -> bool {
        if !self.by_id.contains_key(outfit_id) {
            return false;
        }
        let now = (self.clock)();
        self.set_override(Some(json!({
            "id": outfit_id,
            "until": now.timestamp() + OVERRIDE_SECONDS,
        })));
        self.change_to(outfit_id, if proper { "proper" } else { "quick" })
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "current": self.current(),
            "target": if self.enabled() { Value::String(self.target(None)) } else { Value::Null },
            "pools": {
                "daily": pool_ids(&self.outfits, "daily"),
                "themed": pool_ids(&self.outfits, "themed"),
                "special": pool_ids(&self.outfits, "special"),
            },
            "changing": self.changing.load(Ordering::SeqCst),
        })
    }
}

/// `str(msg.get("id"))` - Python's `str(None)` is `"None"`.
fn msg_id(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => (if *b { "True" } else { "False" }).to_string(),
        Some(other) => other.to_string(),
    }
}
