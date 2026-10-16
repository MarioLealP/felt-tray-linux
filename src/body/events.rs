//! What her body notices, for felt-core's diary/self-memory material (WP-26): `POST /v1/events {kind, data,
//! at}`. One-way and never awaited by her behaviour; sent in order by one task. While felt-core is down the
//! last 200 wait in a buffer and go out oldest first once it answers again. `day_start` (your first activity
//! after 05:00 local each day) is decided here, persisted so a same-day restart doesn't repeat it.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::clock;
use crate::core::FeltCore;

const BUFFER: usize = 200;
const DAY_START_HOUR: i8 = 5;
/// Not activity: going idle, the hourly mood snapshot, day_start itself.
const NOT_ACTIVITY: [&str; 3] = ["afk_start", "day_start", "mood"];

struct Report {
    kind: String,
    data: Value,
    at: String,
}

#[derive(Clone)]
pub struct Events {
    tx: mpsc::UnboundedSender<Report>,
}

impl Events {
    /// Starts the sender on the current runtime; `day_start_file` keeps the last day_start date.
    pub fn start(core: Arc<FeltCore>, day_start_file: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(send(core, rx, day_start_file));
        Self { tx }
    }

    pub fn report(&self, kind: &str, data: Value) {
        let now = clock::local();
        let at = now
            .timestamp()
            .display_with_offset(now.offset())
            .to_string();
        let _ = self.tx.send(Report {
            kind: kind.to_owned(),
            data,
            at,
        });
    }
}

async fn send(core: Arc<FeltCore>, mut rx: mpsc::UnboundedReceiver<Report>, day_file: PathBuf) {
    let mut buffer: VecDeque<Report> = VecDeque::new();
    let mut last_day_start = std::fs::read_to_string(&day_file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["date"].as_str().map(str::to_owned));
    let mut down = false;
    while let Some(r) = rx.recv().await {
        if let Some(today) = day_start_due(&r, last_day_start.as_deref()) {
            if let Err(e) = std::fs::write(&day_file, json!({ "date": today }).to_string()) {
                tracing::warn!("couldn't save day_start: {e}");
            }
            last_day_start = Some(today);
            let at = r.at.clone();
            buffer.push_back(Report {
                kind: "day_start".into(),
                data: json!({}),
                at,
            });
        }
        buffer.push_back(r);
        while buffer.len() > BUFFER {
            buffer.pop_front();
        }
        // oldest first; stop at the first failure so order holds and a down felt-core isn't hammered
        while let Some(next) = buffer.front() {
            match core.report_event(&next.kind, &next.data, &next.at).await {
                Ok(s) if s == StatusCode::NO_CONTENT || s == StatusCode::NOT_FOUND => {
                    if down {
                        tracing::info!("events: felt-core reachable again");
                        down = false;
                    }
                    buffer.pop_front();
                }
                other => {
                    if !down {
                        tracing::warn!("events: felt-core not taking them ({other:?}) - buffering");
                        down = true;
                    }
                    break;
                }
            }
        }
    }
}

/// Today's date if this report is the day's first real activity at/after 05:00.
fn day_start_due(r: &Report, last: Option<&str>) -> Option<String> {
    if NOT_ACTIVITY.contains(&r.kind.as_str()) {
        return None;
    }
    let ts: jiff::Timestamp = r.at.parse().ok()?;
    let local = ts.to_zoned(clock::local().time_zone().clone());
    if local.hour() < DAY_START_HOUR {
        return None;
    }
    let today = local.date().to_string();
    (last != Some(today.as_str())).then_some(today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(kind: &str, local: &str) -> Report {
        let tz = clock::local().time_zone().clone();
        let civil: jiff::civil::DateTime = local.parse().unwrap();
        let z = civil.to_zoned(tz).unwrap();
        Report {
            kind: kind.into(),
            data: json!({}),
            at: z.timestamp().display_with_offset(z.offset()).to_string(),
        }
    }

    #[test]
    fn day_start_once_per_day_after_five() {
        assert_eq!(
            day_start_due(&at("game_start", "2026-10-16T04:30"), None),
            None
        );
        let d = day_start_due(&at("game_start", "2026-10-16T09:00"), None);
        assert_eq!(d.as_deref(), Some("2026-10-16"));
        assert_eq!(
            day_start_due(&at("game_start", "2026-10-16T10:00"), d.as_deref()),
            None
        );
        assert_eq!(
            day_start_due(&at("mood", "2026-10-17T09:00"), d.as_deref()),
            None
        );
        assert!(day_start_due(&at("afk_end", "2026-10-17T09:00"), d.as_deref()).is_some());
    }
}
