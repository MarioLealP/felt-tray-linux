//! The fallback for when felt-core itself is down (Mario 2026-10-03: "app needs to be able to tell if she told
//! me, or if its felt who is down"): the server's alerts straight from ntfy's `clockmost` topic - through Caddy's
//! felt-api host on the tray's client cert, Caddy adding the read-only token (no ntfy login on the desktop).
//! Each alert waits 2 minutes; if felt-core is still down then, it pops up raw, marked as not from her. While
//! felt-core answers it stays silent - her lines cover everything.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::bus::SharedStatus;
use crate::core::FeltCore;
use crate::notify::{Notifier, Popup};

const GRACE: Duration = Duration::from_secs(120);

/// (title, text, urgent)
type Alert = (String, String, bool);
/// Alerts waiting out the grace, with when they came in.
type Held = Arc<Mutex<Vec<(Instant, Alert)>>>;

/// A raw ntfy event as (title, text, urgent), or None for what never pops raw (keepalives, RESOLVED).
fn raw_notice(e: &Value) -> Option<Alert> {
    if e["event"] != "message" {
        return None;
    }
    let title = e["title"].as_str().unwrap_or_default();
    if title.contains("RESOLVED") {
        return None;
    }
    let urgent = e["priority"].as_i64().unwrap_or(3) >= 5;
    let title = if title.is_empty() {
        "server alert"
    } else {
        title
    };
    Some((
        title.to_owned(),
        e["message"].as_str().unwrap_or_default().to_owned(),
        urgent,
    ))
}

pub fn start(core: Arc<FeltCore>, notifier: Notifier, status: Arc<SharedStatus>) {
    let held: Held = Arc::default();
    tokio::spawn(listen(core, held.clone()));
    tokio::spawn(release(notifier, status, held));
}

async fn listen(core: Arc<FeltCore>, held: Held) {
    let mut backoff = Duration::from_secs(5);
    loop {
        match core.raw_alerts().await {
            Ok(mut r) => {
                backoff = Duration::from_secs(5);
                let mut buf = Vec::new();
                while let Ok(Some(chunk)) = r.chunk().await {
                    buf.extend_from_slice(&chunk);
                    while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=nl).collect();
                        if let Some(n) = serde_json::from_slice::<Value>(&line)
                            .ok()
                            .as_ref()
                            .and_then(raw_notice)
                        {
                            // every one waits: felt-core may be going down right now (the alert may be about it)
                            held.lock().await.push((Instant::now(), n));
                        }
                    }
                }
            }
            Err(e) => tracing::debug!("raw alerts: {e:#}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(120));
    }
}

async fn release(notifier: Notifier, status: Arc<SharedStatus>, held: Held) {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let ripe: Vec<Alert> = {
            let mut h = held.lock().await;
            let (ripe, keep): (Vec<_>, Vec<_>) =
                h.drain(..).partition(|(at, _)| at.elapsed() >= GRACE);
            *h = keep;
            ripe.into_iter().map(|(_, n)| n).collect()
        };
        if ripe.is_empty() || status.text() != "offline" {
            continue; // felt-core answers: her own lines cover it
        }
        for (title, text, urgent) in ripe {
            let summary = format!("Server alert (Felt is down): {title}");
            let popup = Popup {
                summary: &summary,
                body: &text,
                actions: &[],
                critical: urgent,
                sticky: true,
                sound: urgent,
            };
            if let Err(e) = notifier.show(&popup).await {
                tracing::warn!("raw alert pop-up failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_real_unresolved_messages_pop() {
        assert!(raw_notice(&json!({"event": "keepalive"})).is_none());
        assert!(raw_notice(&json!({"event": "message", "title": "RESOLVED: disk"})).is_none());
        let (t, m, urgent) = raw_notice(
            &json!({"event": "message", "title": "Caddy down", "message": "x", "priority": 5}),
        )
        .unwrap();
        assert_eq!((t.as_str(), m.as_str(), urgent), ("Caddy down", "x", true));
        assert_eq!(
            raw_notice(&json!({"event": "message"})).unwrap().0,
            "server alert"
        );
    }
}
