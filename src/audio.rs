//! Her voice: WAV clips played one after another through PipeWire (`pw-play -`, stdin). A new reply calls
//! [`Player::stop`] first, which kills the clip that's playing and drops the queue.

use std::collections::VecDeque;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

enum Cmd {
    Play(Vec<u8>),
    Stop,
}

#[derive(Clone)]
pub struct Player {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Player {
    /// Starts the player task; needs a tokio runtime.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(rx));
        Self { tx }
    }

    pub fn play(&self, wav: Vec<u8>) {
        if !wav.is_empty() {
            let _ = self.tx.send(Cmd::Play(wav));
        }
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

async fn run(mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut queue: VecDeque<Vec<u8>> = VecDeque::new();
    let mut current: Option<Child> = None;
    loop {
        if current.is_none()
            && let Some(wav) = queue.pop_front()
        {
            current = start(wav).await;
        }
        let cmd = match current.as_mut() {
            Some(child) => tokio::select! {
                _ = child.wait() => { current = None; continue; }
                cmd = rx.recv() => cmd,
            },
            None => rx.recv().await,
        };
        match cmd {
            None => break,
            Some(Cmd::Play(wav)) => queue.push_back(wav),
            Some(Cmd::Stop) => {
                queue.clear();
                if let Some(mut c) = current.take() {
                    let _ = c.kill().await;
                }
            }
        }
    }
}

async fn start(wav: Vec<u8>) -> Option<Child> {
    // nosemgrep: clockmost-shell-out  fixed binary, fixed args; the WAV goes in on stdin
    let mut child = Command::new("pw-play")
        .args(["--media-role", "Communication", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .inspect_err(|e| tracing::warn!("can't start pw-play: {e}"))
        .ok()?;
    let mut stdin = child.stdin.take()?;
    // Fed from its own task so a long clip never blocks Stop.
    tokio::spawn(async move {
        let _ = stdin.write_all(&wav).await;
    });
    Some(child)
}
