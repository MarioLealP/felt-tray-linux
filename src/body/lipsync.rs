//! Her mouth cues from her voice: Rhubarb Lip Sync, offline, with the reply text as a dialogue hint (faster and
//! more accurate than audio alone). Rhubarb wants files, so the WAV and text go to a private per-user runtime
//! dir for the run and are removed after.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::process::Command;

const RUN_LIMIT: Duration = Duration::from_secs(20);

/// Rhubarb's `mouthCues` (`[{start, end, value}]`), or empty on any failure - she then just doesn't move her
/// mouth, never blocks the reply.
pub async fn cues(rhubarb: &[String], wav: &[u8], text: &str) -> Vec<Value> {
    if wav.is_empty() {
        return Vec::new();
    }
    match run(rhubarb, wav, text).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("lip sync: {e:#}");
            Vec::new()
        }
    }
}

async fn run(rhubarb: &[String], wav: &[u8], text: &str) -> Result<Vec<Value>> {
    let (program, args) = rhubarb.split_first().context("body.rhubarb is empty")?;
    let dir = work_dir()?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let (wav_path, txt_path) = (dir.join(format!("{n}.wav")), dir.join(format!("{n}.txt")));
    let _cleanup = Cleanup(vec![wav_path.clone(), txt_path.clone()]);
    tokio::fs::write(&wav_path, wav).await?;
    tokio::fs::write(&txt_path, text).await?;
    let started = Instant::now();
    // nosemgrep: clockmost-shell-out  the user's configured Rhubarb (no shell) on our own files
    let out = Command::new(program)
        .args(args)
        .args(["-f", "json", "-d"])
        .arg(&txt_path)
        .arg(&wav_path)
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(RUN_LIMIT, out)
        .await
        .context("rhubarb took too long")?
        .with_context(|| format!("can't run {program}"))?;
    if !out.status.success() {
        bail!(
            "rhubarb failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("rhubarb's output isn't JSON")?;
    let cues = v["mouthCues"].as_array().cloned().unwrap_or_default();
    tracing::info!(
        "lip sync: {} cues in {:.2}s",
        cues.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(cues)
}

static NEXT: AtomicU64 = AtomicU64::new(0);

/// `$XDG_RUNTIME_DIR/felt-tray/lipsync` (per-user tmpfs, 0700): never the shared temp dir.
fn work_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let dir = Path::new(&base).join("felt-tray/lipsync");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}
