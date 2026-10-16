//! How she says something: through her body when it's on (it times her mouth and gestures to the sound), else
//! straight to the player. Chat, screen reactions and her own lines all speak through this.

use crate::audio::Player;
use crate::body;

#[derive(Clone)]
pub struct Voice {
    player: Player,
    body: Option<body::Handle>,
}

impl Voice {
    pub fn new(player: Player, body: Option<body::Handle>) -> Self {
        Self { player, body }
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
    /// `prompted`: you asked for it (chat), not one of her own lines or a screen reaction.
    pub fn say(
        &self,
        text: &str,
        audio: Vec<u8>,
        tag: Option<String>,
        emotion: Option<String>,
        prompted: bool,
    ) {
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
