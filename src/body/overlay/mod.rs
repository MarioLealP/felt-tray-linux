//! The relay link to her Godot overlay: one persistent loopback connection, newline-delimited JSON both ways.
//! Persistent on purpose - main.gd broadcasts `{"finished": tag}`, `{"arrived"}`, `{"landed"}` and friends over
//! whatever connection is open, and those notices (not guessed durations) are what her behaviour waits on.
//! [`supervisor`] keeps the overlay running and connected; this file is the shared handle everything else uses.

mod commands;
mod process;
mod rig;
pub mod supervisor;

pub use rig::Rig;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub enum Event {
    /// A fresh connection: whatever the overlay should already know (gaze mode, resting face, outfit, props) is
    /// resent - a restarted overlay starts from nothing.
    Connected,
    Message(Value),
}

/// What main.gd has told us about itself.
#[derive(Debug, Default)]
struct Seen {
    /// The body clip it's playing as far as we know: the last tag sent, until its `finished` notice.
    current_tag: Option<String>,
    /// `{"layers": {tag: "base"|"upper"|"full"}}`, sent on connect.
    layers: HashMap<String, String>,
    /// `{"geometry": {...}}`: her visible frame in world units.
    geometry: Option<Value>,
}

pub struct Overlay {
    writer: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
    events: broadcast::Sender<Event>,
    seen: Mutex<Seen>,
}

impl Overlay {
    pub fn new() -> Self {
        Self {
            writer: tokio::sync::Mutex::new(None),
            events: broadcast::channel(256).0,
            seen: Mutex::default(),
        }
    }

    /// Subscribe *before* sending whatever you'll wait on, so the notice can't slip past.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub async fn connected(&self) -> bool {
        self.writer.lock().await.is_some()
    }

    /// True if the line was written. Never waits for a connection: with the overlay down there's nobody to
    /// tell, so the message is dropped (the supervisor reconnects and [`Event::Connected`] resends state).
    pub async fn send(&self, msg: &Value) -> bool {
        let mut slot = self.writer.lock().await;
        let Some(w) = slot.as_mut() else {
            return false;
        };
        let line = format!("{msg}\n");
        if let Err(e) = w.write_all(line.as_bytes()).await {
            tracing::warn!("lost the overlay mid-send: {e}");
            *slot = None;
            return false;
        }
        if let Some(tag) = msg.get("tag").and_then(Value::as_str) {
            self.seen.lock().expect("seen lock").current_tag = Some(tag.to_owned());
        }
        true
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "gaze reads it (port step 4)"))]
    pub fn current_tag(&self) -> Option<String> {
        self.seen.lock().expect("seen lock").current_tag.clone()
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the world/roam loops read it (port step 4)")
    )]
    pub fn geometry(&self) -> Option<Value> {
        self.seen.lock().expect("seen lock").geometry.clone()
    }

    /// "base" / "upper" / "full" as main.gd reported it; an unreported tag counts as "full" (never two clips
    /// fighting over the same bones).
    pub fn tag_layer(&self, tag: &str) -> String {
        layer_for(tag, &self.seen.lock().expect("seen lock").layers)
    }

    async fn attach(&self, w: OwnedWriteHalf) {
        *self.writer.lock().await = Some(w);
        let _ = self.events.send(Event::Connected);
    }

    async fn detach(&self) {
        *self.writer.lock().await = None;
    }

    /// One line from main.gd: bookkeeping, then out to every subscriber.
    fn received(&self, msg: Value) {
        let mut seen = self.seen.lock().expect("seen lock");
        if let Some(tag) = msg.get("finished").and_then(Value::as_str) {
            if msg.get("missing").and_then(Value::as_bool) == Some(true) {
                tracing::warn!("the overlay has no clips for '{tag}'");
            }
            if seen.current_tag.as_deref() == Some(tag) {
                seen.current_tag = None;
            }
        }
        if msg.get("arrived").and_then(Value::as_bool) == Some(true)
            && seen
                .current_tag
                .as_deref()
                .is_some_and(|t| t.starts_with("locomotion"))
        {
            // a looping walk cycle never sends `finished`; main.gd swaps to idle on arrival
            seen.current_tag = None;
        }
        if let Some(layers) = msg.get("layers").and_then(Value::as_object) {
            seen.layers = layers
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect();
            tracing::info!("the overlay reported layers for {} tags", seen.layers.len());
        }
        if let Some(g) = msg.get("geometry").filter(|g| g.is_object()) {
            tracing::info!("the overlay reported geometry {g}");
            seen.geometry = Some(g.clone());
        }
        drop(seen);
        let _ = self.events.send(Event::Message(msg));
    }
}

/// The next message matching `pred`, or None on timeout (or a lagging/closed channel).
pub async fn wait_for(
    rx: &mut broadcast::Receiver<Event>,
    timeout: Duration,
    pred: impl Fn(&Value) -> bool,
) -> Option<Value> {
    tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Ok(Event::Message(m)) if pred(&m) => return Some(m),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// main.gd's `_is_looping_tag`: every tag starting with "idle" loops, so no `finished` notice ever comes.
pub fn is_looping_tag(tag: &str) -> bool {
    tag.starts_with("idle")
}

fn layer_for(tag: &str, reported: &HashMap<String, String>) -> String {
    reported.get(tag).cloned().unwrap_or_else(|| "full".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_tags_loop() {
        assert!(is_looping_tag("idle"));
        assert!(is_looping_tag("idle/sitting"));
        assert!(!is_looping_tag("reaction/wave"));
    }

    #[test]
    fn finished_clears_the_current_tag() {
        let o = Overlay::new();
        o.seen.lock().unwrap().current_tag = Some("reaction/wave".into());
        o.received(json!({"finished": "reaction/other"}));
        assert_eq!(o.current_tag().as_deref(), Some("reaction/wave"));
        o.received(json!({"finished": "reaction/wave"}));
        assert_eq!(o.current_tag(), None);
    }

    #[test]
    fn arrival_ends_a_walk_cycle() {
        let o = Overlay::new();
        o.seen.lock().unwrap().current_tag = Some("locomotion/walk".into());
        o.received(json!({"arrived": true}));
        assert_eq!(o.current_tag(), None);
    }

    #[test]
    fn layers_and_geometry_are_kept() {
        let o = Overlay::new();
        o.received(json!({"layers": {"reaction/wave": "upper"}, "geometry": {"half_width": 3.2}}));
        assert_eq!(o.tag_layer("reaction/wave"), "upper");
        assert_eq!(o.tag_layer("unknown"), "full");
        assert_eq!(o.geometry().unwrap()["half_width"], 3.2);
    }

    #[tokio::test]
    async fn wait_for_skips_other_messages() {
        let o = Overlay::new();
        let mut rx = o.subscribe();
        o.received(json!({"finished": "a"}));
        o.received(json!({"finished": "b"}));
        let m = wait_for(&mut rx, Duration::from_millis(50), |m| m["finished"] == "b").await;
        assert_eq!(m.unwrap()["finished"], "b");
        assert!(
            wait_for(&mut rx, Duration::from_millis(20), |_| true)
                .await
                .is_none()
        );
    }
}
