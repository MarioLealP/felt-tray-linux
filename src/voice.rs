//! How she says something: through her body when it's on (it times her mouth and gestures to the sound), else
//! straight to the player. Chat, screen reactions and her own lines all speak through this.

use crate::audio::Player;
use crate::body;

/// The last thing she said, as JSON (`text`, `at` unix seconds, `source`): the desktop shell's Felt column reads
/// it over the bus (`LastLine`). "{}" until she says something.
pub type LastLine = std::sync::Arc<std::sync::Mutex<String>>;

#[derive(Clone)]
pub struct Voice {
    player: Player,
    body: Option<body::Handle>,
    last: LastLine,
}

impl Voice {
    pub fn new(player: Player, body: Option<body::Handle>, last: LastLine) -> Self {
        Self { player, body, last }
    }

    pub fn player(&self) -> Player {
        self.player.clone()
    }

    /// A line went to felt-core; her reply is on its way.
    pub fn thinking(&self) {
        if let Some(b) = &self.body {
            b.thinking();
        }
    }

    /// The first (or only) sentence of a reply. `tag`/`emotion`: felt-core's animation tag and emotion.
    /// `prompted`: you asked for it (chat), not one of her own lines or a screen reaction. `source`: what made her
    /// say it ("chat", "ambient", "screen") - shown under the line in the shell's Felt column.
    pub fn say(
        &self,
        text: &str,
        audio: Vec<u8>,
        tag: Option<String>,
        emotion: Option<String>,
        prompted: bool,
        source: &str,
    ) {
        if text.trim().is_empty() && audio.is_empty() {
            return; // nothing to say (felt-core's "no reaction"): no gesture, no face
        }
        if !text.trim().is_empty() {
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            *self.last.lock().expect("last line lock") =
                serde_json::json!({ "text": text.trim(), "at": at, "source": source }).to_string();
        }
        match &self.body {
            Some(b) => b.reply(text, audio, tag, emotion, prompted),
            None => self.player.play(audio),
        }
    }

    /// The Felt app's hide switch, for her body.
    pub fn set_hidden(&self, hidden: bool) {
        if let Some(b) = &self.body {
            b.set_hidden(hidden);
        }
    }

    /// The Felt app's quiet switch, for her body (no spoken reactions nobody asked for).
    pub fn set_quiet(&self, quiet: bool) {
        if let Some(b) = &self.body {
            b.set_quiet(quiet);
        }
    }

    /// Her body's tone line (mood, time of day, recent emotions) for felt-core; None without a body.
    pub fn tone(&self) -> Option<String> {
        self.body
            .as_ref()
            .map(body::Handle::tone)
            .filter(|t| !t.is_empty())
    }

    /// You're talking to her (the talk shortcut): she listens, then thinks.
    pub fn listening(&self, on: bool) {
        if let Some(b) = &self.body {
            b.listening(on);
        }
    }

    /// A reply's desktop action (`{"kind": "presence", "payload": {"action": ...}}`) for her body.
    pub fn ui_action(&self, action: &serde_json::Value) {
        if action["kind"] != "presence" {
            return;
        }
        if let (Some(b), Some(what)) = (&self.body, action["payload"]["action"].as_str()) {
            b.act(what, &action["payload"]);
        }
    }

    /// Is there a body to hide?
    pub fn has_body(&self) -> bool {
        self.body.is_some()
    }

    /// A later sentence of the same reply.
    pub fn more(&self, audio: Vec<u8>) {
        match &self.body {
            Some(b) => b.more("", audio),
            None => self.player.play(audio),
        }
    }
}
