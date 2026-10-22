//! Talking to her: a line typed in the chat window, or spoken (the talk shortcut: press, talk, press again).
//! Send it to felt-core, speak her reply (chunk 0 now, the rest as the stream delivers them); a newer line
//! cancels an older reply's voice. Spoken through [`Voice`] (her body, when it's on). A reply's desktop action
//! (hush, brb, come here ...) goes to her body.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;

use crate::core::{Chunk, FeltCore, Reply};
use crate::mic::Recording;
use crate::voice::Voice;

pub struct Chat {
    core: Arc<FeltCore>,
    voice: Voice,
    session_id: String,
    generation: AtomicU64,
    /// Your voice, while the talk shortcut has it recording.
    recording: tokio::sync::Mutex<Option<Recording>>,
}

impl Chat {
    pub fn new(core: Arc<FeltCore>, voice: Voice, session_id: String) -> Arc<Self> {
        Arc::new(Self {
            core,
            voice,
            session_id,
            generation: AtomicU64::new(0),
            recording: tokio::sync::Mutex::new(None),
        })
    }

    /// The talk shortcut: the first press starts listening (she's cut off if she was talking), the second sends
    /// what you said.
    pub async fn toggle_voice(self: &Arc<Self>) -> Result<()> {
        let mut slot = self.recording.lock().await;
        let Some(rec) = slot.take() else {
            self.generation.fetch_add(1, Ordering::SeqCst); // barge-in ends a streamed reply too
            self.voice.player().stop();
            *slot = Some(Recording::start()?);
            self.voice.listening(true);
            tracing::info!("voice: listening");
            return Ok(());
        };
        drop(slot);
        let wav = rec.stop().await;
        self.voice.listening(false);
        if wav.len() <= 44 {
            tracing::info!("voice: nothing recorded");
            return Ok(());
        }
        let token = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.voice.thinking();
        let reply = self
            .core
            .voice_chat(wav, &self.session_id, self.voice.tone())
            .await?;
        tracing::info!(
            heard = reply.transcribed.len(),
            chars = reply.text.len(),
            "voice reply"
        );
        self.speak(reply, token);
        Ok(())
    }

    /// Returns once her first sentence is ready (the app then refetches the conversation); the rest of a
    /// long reply keeps playing in the background.
    pub async fn send(self: &Arc<Self>, message: &str) -> Result<()> {
        let token = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.voice.player().stop();
        self.voice.thinking();
        let reply = self
            .core
            .text_chat(message, &self.session_id, self.voice.tone())
            .await?;
        tracing::info!(
            chars = reply.text.len(),
            streamed = reply.stream.is_some(),
            "reply"
        );
        self.speak(reply, token);
        Ok(())
    }

    fn speak(self: &Arc<Self>, reply: Reply, token: u64) {
        if self.generation.load(Ordering::SeqCst) != token {
            return; // a newer line went out while this one was waiting
        }
        if let Some(action) = &reply.ui_action {
            self.voice.ui_action(action);
        }
        self.voice
            .say(&reply.text, reply.audio, reply.tag, reply.emotion, true);
        if let Some((id, total)) = reply.stream {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.stream_rest(&id, total, token).await });
        }
    }

    /// The rest of a streamed reply: up to `total` chunks, or for an open one (WP-65) until felt-core says it
    /// ended (204).
    async fn stream_rest(&self, id: &str, total: Option<u32>, token: u64) {
        for n in 1..total.unwrap_or(u32::MAX) {
            let mut tries = 0;
            let audio = loop {
                if self.generation.load(Ordering::SeqCst) != token {
                    return;
                }
                match self.core.reply_chunk(id, n).await {
                    Ok(Chunk::Audio(a)) => break a,
                    Ok(Chunk::NotYet) if tries == 0 => tries += 1,
                    Ok(Chunk::NotYet) => {
                        return tracing::info!("reply {id}: chunk {n} timed out twice");
                    }
                    Ok(Chunk::End(s)) => {
                        return tracing::info!("reply {id} stopped at chunk {n}: {s}");
                    }
                    Err(e) => return tracing::warn!("reply {id} chunk {n}: {e:#}"),
                }
            };
            if self.generation.load(Ordering::SeqCst) != token {
                return;
            }
            self.voice.more(audio);
        }
    }
}
