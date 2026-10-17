//! felt-core over Caddy's machine host: the tray's 2-hour Felt client cert is its identity (WP-62 M2b), no
//! token. The renewer swaps `bundle.pem` about hourly, so the HTTP client is rebuilt whenever the file's
//! mtime changes - new connections always present the current cert.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use reqwest::{Client, Identity, StatusCode};
use serde_json::json;

pub struct FeltCore {
    base: String,
    bundle: PathBuf,
    client: Mutex<Option<(SystemTime, Client)>>,
}

/// felt-core's chat answer: her text, and chunk 0 of her voice (WAV). A long reply streams: `stream` holds
/// the id + chunk count, chunks 1.. come from [`FeltCore::reply_chunk`] (WP-54).
pub struct Reply {
    /// What felt-core heard (voice turns).
    pub transcribed: String,
    /// A skill's desktop action (`{"kind": "presence", "payload": {...}}`: hush, brb, come here ...).
    pub ui_action: Option<serde_json::Value>,
    pub text: String,
    pub audio: Vec<u8>,
    pub stream: Option<(String, u32)>,
    /// A skill's animation tag (e.g. `reaction/weather/storm`), when one fired.
    pub tag: Option<String>,
    /// Her emotion, on screen reactions.
    pub emotion: Option<String>,
}

pub enum Chunk {
    Audio(Vec<u8>),
    /// 204 done, 404 expired, 502 that chunk's TTS failed - stop either way.
    End(StatusCode),
    /// 408: not ready inside the long-poll window.
    NotYet,
}

impl FeltCore {
    pub fn new(base: String, bundle: PathBuf) -> Self {
        Self {
            base,
            bundle,
            client: Mutex::new(None),
        }
    }

    fn client(&self) -> Result<Client> {
        let mtime = std::fs::metadata(&self.bundle)
            .and_then(|m| m.modified())
            .with_context(|| format!("can't read the client cert {}", self.bundle.display()))?;
        let mut slot = self.client.lock().expect("client lock");
        if let Some((seen, c)) = slot.as_ref()
            && *seen == mtime
        {
            return Ok(c.clone());
        }
        let pem = std::fs::read(&self.bundle)?;
        let c = Client::builder()
            .identity(Identity::from_pem(&pem).context("bad client bundle")?)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(90))
            .build()?;
        *slot = Some((mtime, c.clone()));
        Ok(c)
    }

    /// A spoken turn: your WAV -> her reply (same shape as a typed one, plus what she heard).
    pub async fn voice_chat(
        &self,
        wav: Vec<u8>,
        session_id: &str,
        tone: Option<String>,
    ) -> Result<Reply> {
        let file = reqwest::multipart::Part::bytes(wav)
            .file_name("voice.wav")
            .mime_str("audio/wav")?;
        let mut form = reqwest::multipart::Form::new().part("file", file);
        if let Some(t) = tone {
            form = form.text("tone", t);
        }
        let r = self
            .client()?
            .post(format!(
                "{}/v1/voice-chat?session_id={session_id}",
                self.base
            ))
            .multipart(form)
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("felt-core answered {}", r.status());
        }
        reply_from(r).await
    }

    /// `tone`: her body's mood/time-of-day line, which felt-core adds to her prompt (WP-15).
    pub async fn text_chat(
        &self,
        message: &str,
        session_id: &str,
        tone: Option<String>,
    ) -> Result<Reply> {
        let r = self
            .client()?
            .post(format!("{}/v1/text-chat", self.base))
            .json(&json!({ "message": message, "session_id": session_id, "tone": tone }))
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("felt-core answered {}", r.status());
        }
        reply_from(r).await
    }

    pub async fn reply_chunk(&self, stream_id: &str, n: u32) -> Result<Chunk> {
        const WAIT: u64 = 30;
        let r = self
            .client()?
            .get(format!(
                "{}/v1/reply/{stream_id}/chunk/{n}?timeout={WAIT}",
                self.base
            ))
            .timeout(Duration::from_secs(WAIT + 10))
            .send()
            .await?;
        Ok(match r.status() {
            StatusCode::OK => Chunk::Audio(r.bytes().await?.to_vec()),
            StatusCode::REQUEST_TIMEOUT => Chunk::NotYet,
            s @ (StatusCode::NO_CONTENT | StatusCode::NOT_FOUND | StatusCode::BAD_GATEWAY) => {
                Chunk::End(s)
            }
            s => bail!("reply chunk {n}: felt-core answered {s}"),
        })
    }

    pub async fn latest_notice(&self) -> Result<u64> {
        let r = self
            .client()?
            .get(format!("{}/v1/notices/latest", self.base))
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?;
        let v: serde_json::Value = r.json().await?;
        v["id"].as_u64().context("no id in /v1/notices/latest")
    }

    pub async fn notices_after(&self, after: u64) -> Result<Vec<crate::notices::Notice>> {
        let r = self
            .client()?
            .get(format!("{}/v1/notices?after={after}", self.base))
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?;
        Ok(r.json().await?)
    }

    pub async fn mark_handled(&self, incident: u64) -> Result<bool> {
        let r = self
            .client()?
            .post(format!("{}/v1/incidents/{incident}/handled", self.base))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        Ok(r.status().is_success())
    }

    /// A captured window (already cleared by the capture policy) -> her reaction, or None on 503 (nothing
    /// this time).
    pub async fn react_screen(
        &self,
        jpeg: Vec<u8>,
        app: &str,
        title: &str,
        trigger: &str,
        session_id: &str,
        tone: Option<String>,
    ) -> Result<Option<Reply>> {
        let image = reqwest::multipart::Part::bytes(jpeg)
            .file_name("screen.jpg")
            .mime_str("image/jpeg")?;
        let form = reqwest::multipart::Form::new()
            .part("image", image)
            .text("app", app.to_owned())
            .text("title", title.to_owned())
            .text("trigger", trigger.to_owned());
        let form = match tone {
            Some(t) => form.text("tone", t),
            None => form,
        };
        let r = self
            .client()?
            .post(format!(
                "{}/v1/react/screen?session_id={session_id}",
                self.base
            ))
            .multipart(form)
            .send()
            .await?;
        if r.status() == StatusCode::SERVICE_UNAVAILABLE {
            return Ok(None);
        }
        let r = r.error_for_status()?;
        let h = r.headers();
        let text = header(h, "X-Reply-Text").unwrap_or_default();
        let (tag, emotion) = (header(h, "X-Animation-Tag"), header(h, "X-Emotion"));
        Ok(Some(Reply {
            transcribed: String::new(),
            ui_action: None,
            text,
            stream: None,
            tag,
            emotion,
            audio: r.bytes().await?.to_vec(),
        }))
    }

    /// WP-25 long-poll: the next proactive item, or None when nothing came within the window.
    pub async fn proactive_next(&self) -> Result<Option<crate::proactive::Item>> {
        const WAIT: u64 = 25;
        let r = self
            .client()?
            .get(format!("{}/v1/proactive/next?timeout={WAIT}", self.base))
            .timeout(Duration::from_secs(WAIT + 10))
            .send()
            .await?;
        if r.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        Ok(Some(r.error_for_status()?.json().await?))
    }

    pub async fn proactive_audio(&self, id: u64) -> Result<Vec<u8>> {
        let r = self
            .client()?
            .get(format!("{}/v1/proactive/{id}/audio", self.base))
            .send()
            .await?
            .error_for_status()?;
        Ok(r.bytes().await?.to_vec())
    }

    pub async fn proactive_requeue(&self, id: u64) -> Result<()> {
        self.client()?
            .post(format!("{}/v1/proactive/{id}/requeue", self.base))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn controls(&self) -> Result<crate::controls::State> {
        let r = self
            .client()?
            .get(format!("{}/v1/controls", self.base))
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?;
        Ok(r.json().await?)
    }

    pub async fn report_desktop(&self, report: &serde_json::Value) -> Result<()> {
        self.client()?
            .post(format!("{}/v1/controls/desktop", self.base))
            .json(report)
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// The next `days` of her calendar, as felt-core shapes it for the desktop (a JSON array).
    pub async fn calendar_upcoming(&self, days: u32) -> Result<serde_json::Value> {
        let r = self
            .client()?
            .get(format!("{}/v1/calendar/upcoming?days={days}", self.base))
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .error_for_status()?;
        let v: serde_json::Value = r.json().await?;
        Ok(v.get("events")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])))
    }

    /// Her body's gesture for a reply felt-core didn't tag: felt-llm picks from `tags` (her animations on this
    /// desktop). None when it can't (felt-llm offline, no pick, a tag not in the list).
    pub async fn reaction_tag(&self, text: &str, tags: &[String]) -> Result<Option<String>> {
        let r = self
            .client()?
            .post(format!("{}/v1/reaction-tag", self.base))
            .json(&json!({ "text": text, "tags": tags }))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        if !r.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = r.json().await?;
        Ok(v["tag"]
            .as_str()
            .filter(|t| tags.iter().any(|k| k == t))
            .map(str::to_owned))
    }

    /// A desktop event (a game started, back from AFK, ...) -> her spoken line, or None when felt-core keeps
    /// her quiet (its cooldowns and minimum durations decide; an empty body + X-Reaction-Skipped).
    pub async fn react_event(
        &self,
        event: &str,
        details: &serde_json::Value,
        tone: &str,
    ) -> Result<Option<Reply>> {
        let r = self
            .client()?
            .post(format!("{}/v1/react/event", self.base))
            .json(&json!({ "event": event, "details": details, "tone": tone }))
            .send()
            .await?
            .error_for_status()?;
        let h = r.headers();
        let text = header(h, "X-Reply-Text").unwrap_or_default();
        let (tag, emotion) = (header(h, "X-Animation-Tag"), header(h, "X-Emotion"));
        if text.is_empty() && tag.is_none() {
            let why = header(h, "X-Reaction-Skipped").unwrap_or_else(|| "?".into());
            tracing::info!("react/event {event}: no line ({why})");
            return Ok(None);
        }
        Ok(Some(Reply {
            transcribed: String::new(),
            ui_action: None,
            text,
            stream: None,
            tag,
            emotion,
            audio: r.bytes().await?.to_vec(),
        }))
    }

    /// What's playing (Spotify via felt-core): track_id, is_playing, progress_ms, ...
    pub async fn music_now(&self) -> Result<serde_json::Value> {
        let r = self
            .client()?
            .get(format!("{}/v1/music/now", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .await?;
        Ok(r.error_for_status()?.json().await?)
    }

    /// The current track's lyrics: found, chorus {start_ms, end_ms}, timed lines.
    pub async fn music_lyrics(&self) -> Result<serde_json::Value> {
        let r = self
            .client()?
            .get(format!("{}/v1/music/lyrics", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .await?;
        Ok(r.error_for_status()?.json().await?)
    }

    /// felt-core's cached current weather condition: Some("rain" / "sun" / ...), Some(None) while it doesn't
    /// know, None = no such endpoint (404).
    pub async fn weather_now(&self) -> Result<Option<Option<String>>> {
        let r = self
            .client()?
            .get(format!("{}/v1/weather/now", self.base))
            .timeout(Duration::from_secs(10))
            .send()
            .await?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let v: serde_json::Value = r.error_for_status()?.json().await?;
        Ok(Some(v["condition"].as_str().map(str::to_owned)))
    }

    /// One diary event (her body's notices); 204 = taken, 404 = a felt-core without `/v1/events`.
    pub async fn report_event(
        &self,
        kind: &str,
        data: &serde_json::Value,
        at: &str,
    ) -> Result<StatusCode> {
        let r = self
            .client()?
            .post(format!("{}/v1/events", self.base))
            .json(&json!({ "kind": kind, "data": data, "at": at }))
            .timeout(Duration::from_secs(5))
            .send()
            .await?;
        Ok(r.status())
    }

    /// Is she reachable (cert accepted, felt-core up)? The cheapest route the tray may call.
    pub async fn ping(&self) -> bool {
        let Ok(c) = self.client() else { return false };
        c.get(format!("{}/v1/notices/latest", self.base))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }
}

/// A chat answer (typed or spoken): her text, voice, stream, tag and any desktop action.
async fn reply_from(r: reqwest::Response) -> Result<Reply> {
    let h = r.headers();
    let text = header(h, "X-Reply-Text").unwrap_or_default();
    let stream = stream_fields(header(h, "X-Reply-Stream"), header(h, "X-Reply-Chunks"));
    let (tag, emotion) = (header(h, "X-Animation-Tag"), header(h, "X-Emotion"));
    let transcribed = header(h, "X-Transcribed-Text").unwrap_or_default();
    let ui_action = header(h, "X-UI-Action").and_then(|j| serde_json::from_str(&j).ok());
    Ok(Reply {
        transcribed,
        ui_action,
        text,
        stream,
        tag,
        emotion,
        audio: r.bytes().await?.to_vec(),
    })
}

/// Headers are percent-encoded by felt-core (non-ASCII replies).
fn header(h: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    let raw = h.get(name)?.to_str().ok()?;
    Some(percent_decode(raw))
}

fn stream_fields(id: Option<String>, chunks: Option<String>) -> Option<(String, u32)> {
    let n: u32 = chunks?.parse().ok()?;
    let id = id.filter(|s| !s.is_empty())?;
    (n >= 2).then_some((id, n))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_encoded_utf8() {
        assert_eq!(percent_decode("Ol%C3%A1%2C%20Mario"), "Olá, Mario");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
    }

    #[test]
    fn stream_needs_an_id_and_two_chunks() {
        assert_eq!(
            stream_fields(Some("s1".into()), Some("3".into())),
            Some(("s1".into(), 3))
        );
        assert_eq!(stream_fields(Some("s1".into()), Some("1".into())), None);
        assert_eq!(stream_fields(None, Some("3".into())), None);
        assert_eq!(stream_fields(Some("s1".into()), Some("x".into())), None);
    }
}
