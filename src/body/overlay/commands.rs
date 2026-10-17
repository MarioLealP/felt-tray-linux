//! The messages main.gd understands, as typed calls. Each returns false (or just doesn't wait) when the overlay
//! is down - there's nobody to tell.

use std::time::Duration;

use serde_json::{Value, json};

use super::{Overlay, is_looping_tag, wait_for};

impl Overlay {
    /// `looped`: main.gd loops the clip until a walk arrives (no `finished` notice). `clip`: that exact clip
    /// of the tag instead of a random one.
    pub async fn play_tag(&self, tag: &str, looped: bool, clip: Option<&str>) -> bool {
        let mut m = json!({ "tag": tag });
        if looped {
            m["loop"] = true.into();
        }
        if let Some(c) = clip {
            m["clip"] = c.into();
        }
        self.send(&m).await
    }

    /// Plays a one-shot tag and waits for its real `finished` notice; `timeout` is only the safety net for a
    /// dropped connection or a tag with no clips. Looping tags (`idle*`) never finish, so they don't wait.
    pub async fn play_and_wait(&self, tag: &str, timeout: Duration, clip: Option<&str>) {
        let mut rx = self.subscribe();
        if !self.play_tag(tag, false, clip).await || is_looping_tag(tag) {
            return;
        }
        let finished = |m: &Value| m.get("finished").and_then(Value::as_str) == Some(tag);
        if wait_for(&mut rx, timeout, finished).await.is_none() {
            tracing::warn!("no finished notice for '{tag}' within {timeout:?} - continuing");
        }
    }

    /// Like [`Overlay::play_and_wait`] from `start` s into the clip (a segment: the mug sip of drink_water).
    pub async fn play_segment(&self, tag: &str, timeout: Duration, clip: Option<&str>, start: f64) {
        if start <= 0.0 {
            return self.play_and_wait(tag, timeout, clip).await;
        }
        let mut rx = self.subscribe();
        let mut m = json!({ "tag": tag, "start": start });
        if let Some(c) = clip {
            m["clip"] = c.into();
        }
        if !self.send(&m).await {
            return;
        }
        let finished = |m: &Value| m.get("finished").and_then(Value::as_str) == Some(tag);
        if wait_for(&mut rx, timeout, finished).await.is_none() {
            tracing::warn!("no finished notice for '{tag}' within {timeout:?} - continuing");
        }
    }

    /// Walk to world x. `offscreen` lets her leave the visible frame (hide/brb); `running` = run speed.
    pub async fn move_to(&self, x: f64, offscreen: bool, running: bool) -> bool {
        let mut m = json!({ "x": x });
        if offscreen {
            m["offscreen"] = true.into();
        }
        if running {
            m["running"] = true.into();
        }
        self.send(&json!({ "move": m })).await
    }

    /// Waits for main.gd's `arrived` notice (the timeout is only a safety net).
    pub async fn wait_for_arrived(&self, timeout: Duration) {
        let mut rx = self.subscribe();
        if !self.connected().await {
            return;
        }
        let arrived = |m: &Value| m.get("arrived").and_then(Value::as_bool) == Some(true);
        if wait_for(&mut rx, timeout, arrived).await.is_none() {
            tracing::warn!("no arrived notice within {timeout:?} - continuing");
        }
    }

    /// Rhubarb's mouth cues (`[{start, end, value}]`) from now; an empty list eases the mouth back to rest.
    pub async fn lipsync(&self, cues: &[Value]) -> bool {
        self.send(&json!({ "lipsync": cues })).await
    }

    /// The face compositor: `{region: {shape: weight}}`, held `hold` seconds (0 = until replaced).
    pub async fn face(&self, regions: &Value, hold: f64) -> bool {
        self.send(&json!({ "face": { "regions": regions, "hold": hold } }))
            .await
    }

    pub async fn bubble(&self, text: &str, seconds: f64) -> bool {
        self.send(&json!({ "bubble": { "text": text, "seconds": seconds } }))
            .await
    }

    /// Her feet go to this desktop-px height (main.gd falls her there and sends `landed`).
    pub async fn support(&self, y_px: f64) -> bool {
        self.send(&json!({ "support": { "y_px": y_px } })).await
    }

    /// Steps off and falls to the platform at `y_px`; the `landed` payload, or None (timeout / overlay down).
    pub async fn hop(&self, y_px: f64, timeout: Duration) -> Option<Value> {
        let m = json!({ "hop": { "y_px": y_px } });
        self.send_and_wait(&m, "landed", timeout).await
    }

    /// Climbs up to `y_px` (`meters`: the 1 or 2 m clip); the `climbed` payload, or None.
    pub async fn climb(&self, y_px: f64, meters: u32, timeout: Duration) -> Option<Value> {
        let m = json!({ "climb": { "y_px": y_px, "meters": meters } });
        self.send_and_wait(&m, "climbed", timeout).await
    }

    /// Sits her on the ledge at `y_px` (hips pinned there); true once main.gd says she's seated.
    pub async fn ledge_sit(&self, y_px: f64, timeout: Duration) -> bool {
        let m = json!({ "ledge_sit": { "y_px": y_px } });
        let seated = self.send_and_wait(&m, "seated", timeout).await;
        seated.and_then(|v| v.as_bool()) == Some(true)
    }

    pub async fn ledge_stand(&self, timeout: Duration) {
        self.send_and_wait(&json!({ "ledge_stand": {} }), "seated", timeout)
            .await;
    }

    /// Sends `msg` and waits for main.gd's next message carrying `key`; that value, or None.
    async fn send_and_wait(&self, msg: &Value, key: &str, timeout: Duration) -> Option<Value> {
        let mut rx = self.subscribe();
        if !self.send(msg).await {
            return None;
        }
        let m = wait_for(&mut rx, timeout, |m| m.get(key).is_some()).await;
        if m.is_none() {
            tracing::warn!("no '{key}' notice within {timeout:?} - continuing");
        }
        m.and_then(|m| m.get(key).cloned())
    }
}
