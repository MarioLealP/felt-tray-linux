//! Keeps her overlay running and connected: launch, connect (secret first), read until the link drops, then
//! reconnect - or relaunch if the overlay itself died, backing off so a crashing overlay can't spin.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedReadHalf;

use super::Overlay;
use super::process::{self, Launched};

const CONNECT_RETRY: Duration = Duration::from_millis(500);
const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// An overlay that stayed up this long counts as healthy again (the backoff resets).
const HEALTHY_AFTER: Duration = Duration::from_secs(120);

pub async fn run(overlay: Arc<Overlay>, command: Vec<String>, port: u16) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let started = tokio::time::Instant::now();
        match process::launch(&command, port).await {
            Ok(mut l) => {
                attend(&overlay, &mut l, port).await;
                tracing::warn!("her overlay stopped");
            }
            Err(e) => tracing::error!("{e:#}"),
        }
        if started.elapsed() >= HEALTHY_AFTER {
            backoff = BACKOFF_MIN;
        }
        tracing::info!("relaunching her overlay in {backoff:?}");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Connects and reconnects for as long as this overlay process lives.
async fn attend(overlay: &Overlay, l: &mut Launched, port: u16) {
    loop {
        let link = tokio::select! {
            s = connect(port, &l.secret) => s,
            _ = l.child.wait() => return,
        };
        let (read, write) = link.into_split();
        overlay.attach(write).await;
        tracing::info!("connected to her overlay");
        tokio::select! {
            _ = read_lines(overlay, read) => tracing::warn!("the overlay link closed"),
            _ = l.child.wait() => {
                overlay.detach().await;
                return;
            }
        }
        overlay.detach().await;
    }
}

/// Retries until the overlay's listener answers (a fresh Godot takes a few seconds to open it).
async fn connect(port: u16, secret: &str) -> TcpStream {
    loop {
        // nosemgrep: clockmost-network-egress  loopback only: her overlay's relay port
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await
            && s.write_all(format!("{secret}\n").as_bytes()).await.is_ok()
        {
            return s;
        }
        tokio::time::sleep(CONNECT_RETRY).await;
    }
}

async fn read_lines(overlay: &Overlay, read: OwnedReadHalf) {
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(msg) = serde_json::from_str(&line) {
            overlay.received(msg);
        }
    }
}
