//! Her alert lines as desktop pop-ups (WP-42.5). felt-core decides which ones pop, which play a sound and
//! which stay (its notices.py); we poll, show, and act on "details" / "handled". The position survives
//! restarts; after one, at most [`CATCH_UP`] missed lines pop, never a flood.
//! Not ported yet: the raw-ntfy fallback for when felt-core itself is down (needs its own ntfy login).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use tao::event_loop::EventLoopProxy;
use tokio::sync::mpsc;

use crate::UiEvent;
use crate::core::FeltCore;
use crate::notify::{Action, Notifier, Popup};

const POLL: Duration = Duration::from_secs(5);
const CATCH_UP: u64 = 5;

#[derive(Debug, Deserialize)]
pub struct Notice {
    pub message_id: u64,
    pub incident_id: u64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub pop: bool,
    #[serde(default)]
    pub sound: bool,
    #[serde(default)]
    pub sticky: bool,
}

pub fn start(
    core: Arc<FeltCore>,
    (notifier, clicks): (Notifier, mpsc::UnboundedReceiver<Action>),
    proxy: EventLoopProxy<UiEvent>,
    state: PathBuf,
) {
    // notification id -> incident id, for the action clicks
    let shown: Arc<Mutex<HashMap<u32, u64>>> = Arc::default();
    tokio::spawn(poll(core.clone(), notifier, shown.clone(), state));
    tokio::spawn(actions(core, shown, proxy, clicks));
}

async fn poll(
    core: Arc<FeltCore>,
    notifier: Notifier,
    shown: Arc<Mutex<HashMap<u32, u64>>>,
    state: PathBuf,
) {
    let mut last: Option<u64> = None;
    loop {
        if last.is_none() {
            last = match core.latest_notice().await {
                Ok(latest) => Some(start_point(load_last(&state), latest)),
                Err(e) => {
                    tracing::debug!("notices: felt-core not answering: {e:#}");
                    None
                }
            };
        }
        if let Some(from) = last {
            match core.notices_after(from).await {
                Ok(lines) => {
                    let mut pos = from;
                    for n in &lines {
                        if n.pop {
                            show(&notifier, &shown, n).await;
                        }
                        pos = pos.max(n.message_id);
                    }
                    if pos != from {
                        save_last(&state, pos);
                        last = Some(pos);
                    }
                }
                Err(e) => tracing::debug!("notices: {e:#}"),
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn show(notifier: &Notifier, shown: &Mutex<HashMap<u32, u64>>, n: &Notice) {
    let summary = match n.area.as_deref() {
        Some(a) if !a.is_empty() => format!("Felt · {a}"),
        _ => "Felt".to_owned(),
    };
    let body = if n.text.is_empty() {
        n.title.clone().unwrap_or_default()
    } else {
        n.text.clone()
    };
    let popup = Popup {
        summary: &summary,
        body: &body,
        actions: &[
            ("default", "Details"),
            ("details", "Details"),
            ("handled", "Handled"),
        ],
        critical: n.tier == "urgent",
        sticky: n.sticky,
        sound: n.sound,
    };
    match notifier.show(&popup).await {
        Ok(id) => {
            shown.lock().expect("shown lock").insert(id, n.incident_id);
        }
        Err(e) => tracing::warn!("pop-up for notice {} failed: {e}", n.message_id),
    }
}

async fn actions(
    core: Arc<FeltCore>,
    shown: Arc<Mutex<HashMap<u32, u64>>>,
    proxy: EventLoopProxy<UiEvent>,
    mut clicks: mpsc::UnboundedReceiver<Action>,
) {
    while let Some((id, key)) = clicks.recv().await {
        let Some(incident) = shown.lock().expect("shown lock").get(&id).copied() else {
            continue; // not one of ours
        };
        match key.as_str() {
            "default" | "details" => {
                let _ = proxy.send_event(UiEvent::OpenAt(format!("#systems/{incident}")));
            }
            "handled" => match core.mark_handled(incident).await {
                Ok(true) => tracing::info!("incident {incident} marked handled"),
                Ok(false) => tracing::warn!("incident {incident}: felt-core refused 'handled'"),
                Err(e) => tracing::warn!("incident {incident} handled: {e:#}"),
            },
            _ => {}
        }
    }
}

/// First run: nothing old pops. After a restart: the few missed ones, never a flood.
fn start_point(last: Option<u64>, latest: u64) -> u64 {
    match last {
        None => latest,
        Some(l) => l.max(latest.saturating_sub(CATCH_UP)),
    }
}

fn load_last(path: &PathBuf) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["last"].as_u64()
}

fn save_last(path: &PathBuf, last: u64) {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::json!({ "last": last }).to_string())
    };
    if let Err(e) = write() {
        tracing::warn!("couldn't save the pop-up position ({e}); a restart may repeat one");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_point_never_floods() {
        assert_eq!(start_point(None, 100), 100);
        assert_eq!(start_point(Some(98), 100), 98);
        assert_eq!(start_point(Some(10), 100), 95);
        assert_eq!(start_point(Some(3), 2), 3);
    }

    #[test]
    fn position_round_trips() {
        let dir = std::env::temp_dir().join(format!("felt-notices-{}", std::process::id()));
        let p = dir.join("notices.json");
        assert_eq!(load_last(&p), None);
        save_last(&p, 42);
        assert_eq!(load_last(&p), Some(42));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_a_felt_core_line() {
        let n: Notice = serde_json::from_str(
            r#"{"message_id": 7, "incident_id": 3, "text": "Caddy is down.", "title": "caddy",
                "area": "server", "tier": "urgent", "kind": "line", "at": "2026-10-15T20:00:00",
                "pop": true, "sound": true, "sticky": true}"#,
        )
        .unwrap();
        assert!(n.pop && n.sticky && n.tier == "urgent");
    }
}
