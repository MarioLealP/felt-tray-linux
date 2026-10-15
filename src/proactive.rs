//! Her own lines (WP-25): felt-core's proactive channel - calendar heads-ups, alarms, things she noticed.
//! Long-polls `/v1/proactive/next`, fetches the item's voice and plays it. If the voice can't be had (felt-voice
//! down), the line pops up as a notification instead, so it is never silently lost. While she's quiet or
//! hidden (the app's switches) items go back to felt-core, except urgent ones.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::audio::Player;
use crate::controls::Switches;
use crate::core::FeltCore;
use crate::notify::{Notifier, Popup};

const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// After handing an item back (quiet / hidden): don't spin on it.
const HELD_PAUSE: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
pub struct Item {
    pub id: u64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub kind: String,
}

pub async fn run(
    core: Arc<FeltCore>,
    player: Player,
    notifier: Option<Notifier>,
    switches: Arc<Switches>,
) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let item = match core.proactive_next().await {
            Ok(Some(item)) => item,
            Ok(None) => continue, // the long-poll's normal "nothing yet"
            Err(e) => {
                tracing::debug!("proactive: felt-core not answering ({e:#}), retry in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = MIN_BACKOFF;
        if !switches.unprompted_ok() && item.kind != "urgent" {
            // quiet / hidden: hand it back, felt-core keeps it for later
            if let Err(e) = core.proactive_requeue(item.id).await {
                tracing::warn!("proactive: couldn't hand item {} back: {e:#}", item.id);
            }
            tokio::time::sleep(HELD_PAUSE).await;
            continue;
        }
        match core.proactive_audio(item.id).await {
            Ok(audio) if !audio.is_empty() => {
                tracing::info!(
                    "proactive: item {} ({}, {} chars)",
                    item.id,
                    item.kind,
                    item.text.len()
                );
                player.play(audio);
            }
            other => {
                if let Err(e) = other {
                    tracing::warn!(
                        "proactive: no voice for item {} ({e:#}) - showing it instead",
                        item.id
                    );
                }
                if let Some(n) = &notifier {
                    let popup = Popup {
                        summary: "Felt",
                        body: &item.text,
                        actions: &[],
                        critical: item.kind == "urgent",
                        sticky: item.kind == "urgent",
                        sound: false,
                    };
                    if let Err(e) = n.show(&popup).await {
                        tracing::warn!(
                            "proactive: item {} lost - no voice and no notification ({e})",
                            item.id
                        );
                    }
                }
            }
        }
    }
}
