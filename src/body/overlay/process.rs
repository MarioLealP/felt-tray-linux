//! Her Godot overlay as a child process. Every launch hands it a fresh secret on stdin (never argv, env, disk or
//! the log) and a relay client's first line must be that secret (WP-62 M2b B5.6). The overlay quits when its
//! stdin closes, so it never outlives felt-tray, and an old one can't be adopted: we wait for the port to free.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin, Command};

const RELAY_SECRET_ARG: &str = "--relay-secret-stdin";
const PORT_FREE_WAIT: Duration = Duration::from_secs(5);

pub struct Launched {
    pub child: Child,
    /// Held open for the overlay's whole life: closing it is the overlay's cue to quit.
    _stdin: ChildStdin,
    pub secret: String,
}

/// `command` is the configured overlay command; the relay port and the secret flag go after `--` as Godot
/// user args.
pub async fn launch(command: &[String], port: u16) -> Result<Launched> {
    let (program, args) = command.split_first().context("body.command is empty")?;
    if !port_free(port).await {
        bail!("something still listens on 127.0.0.1:{port} - not launching a second overlay");
    }
    let secret = new_secret()?;
    let mut args = args.to_vec();
    if !args.iter().any(|a| a == "--") {
        args.push("--".into());
    }
    args.push(format!("--port={port}"));
    args.push(RELAY_SECRET_ARG.into());
    // nosemgrep: clockmost-shell-out  the user's configured overlay command (no shell); the secret goes on stdin
    let mut child = Command::new(program)
        .args(&args)
        // her sound effects go into the same "Felt" mixer stream as her voice (audio.rs; PulseAudio clients
        // honour PULSE_SINK, and fall back to the default output if it isn't there)
        .env("PULSE_SINK", "felt-voice")
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("can't start the overlay ({program})"))?;
    let mut stdin = child.stdin.take().context("no stdin pipe to the overlay")?;
    stdin
        .write_all(format!("{secret}\n").as_bytes())
        .await
        .context("couldn't hand the overlay its launch secret")?;
    stdin.flush().await?;
    tracing::info!("launched her overlay (pid {:?})", child.id());
    Ok(Launched {
        child,
        _stdin: stdin,
        secret,
    })
}

/// 32 random bytes, hex: the per-launch relay secret.
fn new_secret() -> Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b)
        .map_err(|e| anyhow::anyhow!("no randomness for the relay secret: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

async fn port_free(port: u16) -> bool {
    let deadline = tokio::time::Instant::now() + PORT_FREE_WAIT;
    loop {
        // nosemgrep: clockmost-network-egress  loopback only: is an old overlay still holding the relay port?
        let busy = TcpStream::connect(("127.0.0.1", port)).await.is_ok();
        if !busy {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_long_and_fresh() {
        let (a, b) = (new_secret().unwrap(), new_secret().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
