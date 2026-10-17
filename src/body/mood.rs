//! Her mood (WP-15): energy follows the hour, boredom creeps up, annoyance fades; talking to her and her own
//! spoken reactions nudge it. Ticked every 30 s and saved, so a restart keeps it (it partly resets each day on
//! its own). Her resting face - what she shows when no pose holds her face - follows it, and an hourly
//! snapshot goes to felt-core's diary material.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use felt_mood::MoodState;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use super::clock;
use super::ctx::Ctx;
use super::overlay::Event;

pub type Mood = MoodState<fn() -> f64, fn() -> felt_mood::DateTime>;

const TICK: Duration = Duration::from_secs(30);
const REPORT_EVERY: u32 = 120; // ticks: hourly

pub fn load(path: &Path) -> Mood {
    let saved = felt_mood::load_mood_state(path);
    MoodState::new(
        Some(saved),
        clock::monotonic_s as fn() -> f64,
        clock::mood_now as fn() -> felt_mood::DateTime,
    )
}

/// `tone`: where her current tone line is published for felt-tray's chat and screen reactions.
pub async fn run(ctx: Rc<Ctx>, path: PathBuf, tone: std::sync::Arc<std::sync::Mutex<String>>) {
    let mut tick = tokio::time::interval(TICK);
    let mut events = ctx.overlay.subscribe();
    let mut sent: Option<Value> = None;
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                ticks += 1;
                let values = {
                    let mut m = ctx.mood.borrow_mut();
                    m.tick();
                    m.as_dict()
                };
                *tone.lock().expect("tone lock") = super::react::tone(&ctx);
                if let Err(e) = felt_mood::save_mood_state(&values, &path) {
                    tracing::warn!("couldn't save her mood: {e}");
                }
                if ticks.is_multiple_of(REPORT_EVERY) {
                    ctx.events.report("mood", json!(values));
                }
            }
            e = events.recv() => match e {
                Ok(Event::Connected) => sent = None,
                Ok(Event::Message(_)) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
        }
        let face = resting_face(&ctx);
        if sent.as_ref() != Some(&face) && ctx.overlay.send(&json!({ "resting_face": face })).await
        {
            tracing::debug!("resting face -> {face}");
            sent = Some(face);
        }
    }
}

/// The faint mood-driven face main.gd shows on any region no pose holds.
fn resting_face(ctx: &Ctx) -> Value {
    let mood: HashMap<String, f64> = ctx.mood.borrow().as_dict().into_iter().collect();
    json!(felt_faces::resting_face_for_mood(&mood))
}
