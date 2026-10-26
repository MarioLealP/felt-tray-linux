//! Felt's eyes on this desktop (cm-qplv): a localhost llama-server (Qwen3-VL-2B on the desktop GPU, the
//! `felt-vision` user service) describes the captured window, and felt-core gets that text instead of the
//! image. Only while the `[vision]` table is in the config - without it the image goes to felt-core as before.
//!
//! The GPU is the games' first: a description is only asked for while enough VRAM is free that loading the
//! model still leaves `margin_mb` for whatever else is running (a 2D game leaves room, a slicer eating 10 GB
//! doesn't). The server sleeps (frees its VRAM) when idle, so an asleep model needs room for itself too.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config;

// Same prompts as felt-core's vision.py (keep them in step).
const SCREEN_PROMPT: &str = "The active window is {app} - \"{title}\". In at most 2 short sentences (under 50 \
words): what is happening, and the most important visible text quoted exactly. Never guess names or text you \
cannot read - don't name a game, site or app unless the window title or the screen shows its name.";
const POE_PROMPT: &str = "The active window is {app} - \"{title}\" (Path of Exile 2). If an item tooltip is \
visible, transcribe it line by line exactly as written, its name first; preserve spelling, numbers and signs, \
and write [unreadable] for text you cannot read. Otherwise describe what is happening in at most 2 short \
sentences, quoting visible text (a boss name, a message) exactly. Never guess.";
const SCREEN_MAX_TOKENS: u32 = 90;
const POE_MAX_TOKENS: u32 = 260;

pub struct Vision {
    cfg: config::Vision,
    http: reqwest::Client,
}

impl Vision {
    pub fn new(cfg: config::Vision) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_s))
            .build()?;
        Ok(Self { cfg, http })
    }

    /// Why not to describe right now (`None` = go ahead).
    pub async fn blocked(&self) -> Option<String> {
        let free = match free_vram_mb(&self.cfg.vram_dir) {
            Ok(f) => f,
            Err(e) => return Some(format!("can't read VRAM: {e:#}")),
        };
        let asleep = match self.asleep().await {
            Ok(a) => a,
            Err(e) => return Some(format!("felt-vision not reachable: {e:#}")),
        };
        let need = need_mb(asleep, self.cfg.model_mb, self.cfg.margin_mb);
        (free < need).then(|| format!("VRAM {free} MB free < {need} MB needed"))
    }

    async fn asleep(&self) -> Result<bool> {
        let props: Value = self
            .http
            .get(format!("{}/props", self.cfg.url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(props["is_sleeping"].as_bool().unwrap_or(false))
    }

    /// The description of the screenshot (a tooltip keeps its lines, anything else is one line).
    pub async fn describe(&self, jpeg: &[u8], app: &str, title: &str) -> Result<String> {
        let poe = is_poe(app);
        let prompt = if poe { POE_PROMPT } else { SCREEN_PROMPT }
            .replace("{app}", app)
            .replace("{title}", title);
        let image = format!("data:image/jpeg;base64,{}", base64(jpeg));
        let body = json!({
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": image}},
                {"type": "text", "text": prompt},
            ]}],
            "max_tokens": if poe { POE_MAX_TOKENS } else { SCREEN_MAX_TOKENS },
            "temperature": 0,
        });
        let reply: Value = self
            .http
            .post(format!("{}/v1/chat/completions", self.cfg.url))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let text = reply["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .trim();
        anyhow::ensure!(!text.is_empty(), "felt-vision described nothing");
        Ok(if poe {
            text.to_owned()
        } else {
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        })
    }
}

/// felt-core's `poe_reactions.is_poe`.
fn is_poe(app: &str) -> bool {
    app.trim().to_lowercase().starts_with("pathofexile")
}

fn need_mb(asleep: bool, model_mb: u64, margin_mb: u64) -> u64 {
    if asleep {
        model_mb + margin_mb
    } else {
        margin_mb
    }
}

fn free_vram_mb(dir: &std::path::Path) -> Result<u64> {
    let read = |name: &str| -> Result<u64> {
        let p = dir.join(name);
        let s = std::fs::read_to_string(&p).with_context(|| format!("{}", p.display()))?;
        Ok(s.trim().parse::<u64>()?)
    };
    let (total, used) = (read("mem_info_vram_total")?, read("mem_info_vram_used")?);
    Ok(total.saturating_sub(used) / (1024 * 1024))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn an_asleep_model_needs_room_for_itself_too() {
        assert_eq!(need_mb(true, 2048, 1536), 3584);
        assert_eq!(need_mb(false, 2048, 1536), 1536);
    }

    #[test]
    fn free_vram_from_sysfs() {
        let dir = std::env::temp_dir().join(format!("felt-vram-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mem_info_vram_total"), "10720641024\n").unwrap();
        std::fs::write(dir.join("mem_info_vram_used"), "3723141120\n").unwrap();
        assert_eq!(free_vram_mb(&dir).unwrap(), 6673);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn poe_like_felt_core() {
        assert!(is_poe(" PathOfExile2.exe"));
        assert!(!is_poe("firefox"));
    }
}
