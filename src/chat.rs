//! A line typed in the chat window: send it to felt-core, speak her reply (chunk 0 now, the rest as the
//! stream delivers them). A newer line cancels an older reply's voice.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;

use crate::audio::Player;
use crate::core::{Chunk, FeltCore};

pub struct Chat {
    core: Arc<FeltCore>,
    player: Player,
    session_id: String,
    generation: AtomicU64,
}

impl Chat {
    pub fn new(core: Arc<FeltCore>, player: Player, session_id: String) -> Arc<Self> {
        Arc::new(Self {
            core,
            player,
            session_id,
            generation: AtomicU64::new(0),
        })
    }

    /// Returns once her first sentence is ready (the app then refetches the conversation); the rest of a
    /// long reply keeps playing in the background.
    pub async fn send(self: &Arc<Self>, message: &str) -> Result<()> {
        let token = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.player.stop();
        let reply = self.core.text_chat(message, &self.session_id).await?;
        tracing::info!(
            chars = reply.text.len(),
            streamed = reply.stream.is_some(),
            "reply"
        );
        if self.generation.load(Ordering::SeqCst) != token {
            return Ok(()); // a newer line went out while this one was waiting
        }
        self.player.play(reply.audio);
        if let Some((id, total)) = reply.stream {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.stream_rest(&id, total, token).await });
        }
        Ok(())
    }

    async fn stream_rest(&self, id: &str, total: u32, token: u64) {
        for n in 1..total {
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
            self.player.play(audio);
        }
    }
}
