//! Her reactions to desktop events (WP-15/WP-17): a mood-weighted, no-immediate-repeat pick from the reaction
//! table (data/reactions.json) - its face, and its body clip when this desktop has it - and, when the row says
//! `speak`, a spoken line from felt-core (`/v1/react/event`; felt-core owns cooldowns and minimum durations).
//! Nothing while she's hidden; no spoken line while the quiet switch is on.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use felt_reaction_table::{Entry, entry_key, pick_reaction};
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde_json::Value;

use super::clock;
use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::{presence, speech};

/// How long an event's face holds.
const FACE_HOLD_S: f64 = 2.0;

/// An event: the table's face/body now, a spoken line in the background when the row wants one.
pub async fn signal_event(ctx: &Rc<Ctx>, key: &str, details: Value) {
    let Some(picked) = fire(ctx, key).await else {
        return;
    };
    let speak = picked.get("speak").and_then(Value::as_bool) == Some(true);
    if speak && !ctx.quiet.get() {
        let (c, key) = (ctx.clone(), key.to_owned());
        tokio::task::spawn_local(async move { spoken_line(&c, &key, &details).await });
    }
}

/// Just the table's face/body for `key` (music events: no spoken line).
pub async fn reaction(ctx: &Rc<Ctx>, key: &str) {
    fire(ctx, key).await;
}

async fn fire(ctx: &Rc<Ctx>, key: &str) -> Option<Entry> {
    if !presence::visible(ctx) {
        return None;
    }
    let picked = pick(ctx, key)?;
    if let Some(pose) = picked.get("pose").and_then(Value::as_str) {
        ctx.apply_pose(pose, FACE_HOLD_S).await;
    }
    let tag = picked.get("tag").and_then(Value::as_str);
    if let Some(tag) = felt_library::resolve_known_tag(tag, &ctx.assets.tags) {
        let (c, tag) = (ctx.clone(), tag.to_owned());
        tokio::task::spawn_local(async move {
            c.behavior
                .run_sequence(&[tag.as_str()], CLIP_TIMEOUT_S, None)
                .await;
        });
    }
    Some(picked)
}

fn pick(ctx: &Rc<Ctx>, key: &str) -> Option<Entry> {
    let entries = ctx.assets.reactions.get(key)?;
    let mood: HashMap<String, f64> = {
        let mut m = ctx.mood.borrow_mut();
        m.tick();
        m.as_dict().into_iter().collect()
    };
    let last = ctx.last_pick.borrow().get(key).cloned();
    let mut rng = StdRng::from_rng(&mut rand::rng());
    let picked = pick_reaction(entries, &mood, last.as_deref(), &mut rng)?.clone();
    if let Some(item) = picked.get("prop").and_then(Value::as_str) {
        // the row's prop (afk_start's book ...): its "reaction:<item>" episode, in the background
        super::props::start_event(ctx, &format!("reaction:{item}"));
    }
    ctx.last_pick
        .borrow_mut()
        .insert(key.to_owned(), entry_key(&picked));
    Some(picked)
}

/// WP-15: the line felt-core appends to her system prompt - time of day, mood, a streak of same-feeling
/// reactions.
pub fn tone(ctx: &Ctx) -> String {
    let mood: BTreeMap<String, f64> = {
        let mut m = ctx.mood.borrow_mut();
        m.tick();
        m.as_dict()
    };
    let streak = ctx.streak.borrow();
    let refs: Vec<&str> = streak.iter().map(String::as_str).collect();
    felt_tone::build_tone_line(&mood, clock::local().hour().into(), Some(&refs))
}

async fn spoken_line(ctx: &Rc<Ctx>, event: &str, details: &Value) {
    match ctx.core.react_event(event, details, &tone(ctx)).await {
        Ok(Some(r)) if presence::visible(ctx) => {
            tracing::info!("react/event {event}: {} chars", r.text.len());
            speech::say(ctx, r.text, r.audio, r.tag, r.emotion, false);
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("react/event {event}: {e:#}"),
    }
}
