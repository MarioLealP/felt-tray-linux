//! Your voice, for talking to her (push-to-talk, toggled): PipeWire's `pw-record` on the default microphone,
//! 16 kHz mono 16-bit into memory until you press again, then one WAV for felt-core's /v1/voice-chat. Nothing is
//! written to disk.

use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

const RATE: u32 = 16_000;

pub struct Recording {
    child: Child,
    reader: JoinHandle<Vec<u8>>,
}

impl Recording {
    pub fn start() -> Result<Self> {
        // nosemgrep: clockmost-shell-out  PipeWire's recorder on the default mic (no shell); audio stays in memory
        let mut child = Command::new("pw-record")
            .args([
                "--raw",
                "--rate",
                &RATE.to_string(),
                "--channels",
                "1",
                "--format",
                "s16",
            ])
            .args([
                "-P",
                "{ media.name=\"Talking to Felt\" media.role=Communication }",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("can't start pw-record for the mic")?;
        let mut out = child.stdout.take().context("no pw-record stdout")?;
        let reader = tokio::spawn(async move {
            let mut pcm = Vec::new();
            let _ = out.read_to_end(&mut pcm).await;
            pcm
        });
        Ok(Self { child, reader })
    }

    /// Stops recording; the WAV (empty if nothing came through).
    pub async fn stop(mut self) -> Vec<u8> {
        let _ = self.child.kill().await;
        let pcm = self.reader.await.unwrap_or_default();
        if pcm.is_empty() {
            Vec::new()
        } else {
            wav(&pcm)
        }
    }
}

/// 16-bit mono PCM -> a WAV file.
fn wav(pcm: &[u8]) -> Vec<u8> {
    let data = &pcm[..pcm.len() / 2 * 2];
    let mut v = Vec::with_capacity(44 + data.len());
    v.extend(b"RIFF");
    v.extend((36 + data.len() as u32).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes()); // PCM
    v.extend(1u16.to_le_bytes()); // mono
    v.extend(RATE.to_le_bytes());
    v.extend((RATE * 2).to_le_bytes());
    v.extend(2u16.to_le_bytes());
    v.extend(16u16.to_le_bytes());
    v.extend(b"data");
    v.extend((data.len() as u32).to_le_bytes());
    v.extend(data);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_pcm_in_a_wav_header() {
        let w = wav(&[1, 0, 2, 0, 3]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 4); // the odd byte dropped
        assert_eq!(w.len(), 48);
    }
}
