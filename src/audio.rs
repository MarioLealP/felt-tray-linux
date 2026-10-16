//! Her voice: WAV clips played one after another by the configured player (default PipeWire's `pw-play -`,
//! WAV on stdin). A new reply calls [`Player::stop`] first, which kills the clip that's playing and drops the
//! queue. [`Player::play_then`] reports the moment a clip really starts (her body times her mouth to it).

use std::collections::VecDeque;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use tokio::sync::oneshot;

enum Cmd {
    Play(Clip),
    Stop,
}

struct Clip {
    wav: Vec<u8>,
    /// Fired when the player process starts on this clip; dropped unfired if the clip is stopped first.
    started: Option<oneshot::Sender<()>>,
}

#[derive(Clone)]
pub struct Player {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Player {
    /// Starts the player task; needs a tokio runtime.
    pub fn spawn(command: Vec<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(rx, command));
        Self { tx }
    }

    pub fn play(&self, wav: Vec<u8>) {
        if !wav.is_empty() {
            let _ = self.tx.send(Cmd::Play(Clip { wav, started: None }));
        }
    }

    /// Queues `wav`; the receiver resolves when it starts playing, or errs if it never does (stopped, empty).
    pub fn play_then(&self, wav: Vec<u8>) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        if !wav.is_empty() {
            let _ = self.tx.send(Cmd::Play(Clip {
                wav,
                started: Some(tx),
            }));
        }
        rx
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

async fn run(mut rx: mpsc::UnboundedReceiver<Cmd>, command: Vec<String>) {
    let mut queue: VecDeque<Clip> = VecDeque::new();
    let mut current: Option<Child> = None;
    loop {
        if current.is_none()
            && let Some(clip) = queue.pop_front()
        {
            current = start(&command, clip.wav).await;
            if current.is_some()
                && let Some(started) = clip.started
            {
                let _ = started.send(());
            }
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
            Some(Cmd::Play(clip)) => queue.push_back(clip),
            Some(Cmd::Stop) => {
                queue.clear();
                if let Some(mut c) = current.take() {
                    let _ = c.kill().await;
                }
            }
        }
    }
}

async fn start(command: &[String], wav: Vec<u8>) -> Option<Child> {
    let (program, args) = command.split_first()?;
    // nosemgrep: clockmost-shell-out  the user's configured player (no shell); the WAV goes in on stdin
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .inspect_err(|e| tracing::warn!("can't start the player {program}: {e}"))
        .ok()?;
    let mut stdin = child.stdin.take()?;
    // Fed from its own task so a long clip never blocks Stop.
    tokio::spawn(async move {
        let _ = stdin.write_all(&wav).await;
    });
    Some(child)
}
