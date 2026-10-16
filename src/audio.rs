//! Her voice: WAV clips played one after another by the configured player (default PipeWire's `pw-play -`,
//! WAV on stdin). A new reply calls [`Player::stop`] first, which kills the clip that's playing and drops the
//! queue. [`Player::play_then`] reports the moment a clip really starts (her body times her mouth to it).

use std::collections::VecDeque;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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
    /// Her voice's volume for the next clips (f32 bits; 1.0 = as recorded) - ducked under a call.
    gain: Arc<AtomicU32>,
}

impl Player {
    /// Starts the player task; needs a tokio runtime.
    pub fn spawn(command: Vec<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        tokio::spawn(run(rx, command, gain.clone()));
        Self { tx, gain }
    }

    /// Volume for the clips that start from now on (`pw-play --volume`; other players ignore it).
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
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

async fn run(mut rx: mpsc::UnboundedReceiver<Cmd>, command: Vec<String>, gain: Arc<AtomicU32>) {
    let mut queue: VecDeque<Clip> = VecDeque::new();
    let mut current: Option<Child> = None;
    loop {
        if current.is_none()
            && let Some(clip) = queue.pop_front()
        {
            current = start(
                &command,
                clip.wav,
                f32::from_bits(gain.load(Ordering::Relaxed)),
            )
            .await;
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

async fn start(command: &[String], wav: Vec<u8>, gain: f32) -> Option<Child> {
    let (program, args) = command.split_first()?;
    let volume = (program.ends_with("pw-play") && (gain - 1.0).abs() > f32::EPSILON)
        .then(|| format!("--volume={gain:.3}"));
    // nosemgrep: clockmost-shell-out  the user's configured player (no shell); the WAV goes in on stdin
    let mut child = Command::new(program)
        .args(volume)
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
