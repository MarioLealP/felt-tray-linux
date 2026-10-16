//! Settings from `$XDG_CONFIG_HOME/felt-tray/config.toml` (or `FELT_TRAY_CONFIG`). No hosts or paths are
//! built in (Felt rule: no hardcoding) - a missing file is an error that says what to write.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// felt-core's machine host (Caddy requires the Felt client cert there), e.g. `https://felt-api.example`.
    pub felt_core_url: String,
    /// The hosted Felt app the chat window opens (behind SSO).
    pub app_url: String,
    /// The renewer's `bundle.pem` (key + cert + intermediate in one file, replaced every ~hour).
    pub client_bundle: PathBuf,
    /// Hosts the chat window may navigate to (the app's own host + the SSO login host). Anything else opens
    /// in the default browser.
    pub window_hosts: Vec<String>,
    /// felt-core session id for typed lines (the app's own session, so her marks land there too).
    #[serde(default = "default_session")]
    pub session_id: String,
    /// Show the tray icon. Off when the desktop shell has its own Felt button (it calls the D-Bus API instead).
    #[serde(default = "default_true")]
    pub tray_icon: bool,
    /// Her alert lines as desktop notifications.
    #[serde(default = "default_true")]
    pub notices: bool,
    /// Chat window size (logical pixels).
    #[serde(default = "default_window_size")]
    pub window_size: [f64; 2],
    /// Plays her voice: a command that reads one WAV on stdin.
    #[serde(default = "default_player")]
    pub player: Vec<String>,
    /// Days of her calendar to offer the shell (D-Bus `Agenda`); 0 = off.
    #[serde(default = "default_agenda_days")]
    pub agenda_days: u32,
    /// Her own lines (felt-core's proactive channel), spoken.
    #[serde(default = "default_true")]
    pub proactive: bool,
    /// React to what's on screen (KDE Plasma/KWin only). Off unless asked for.
    #[serde(default)]
    pub screen_reactions: bool,
    /// The privacy blocklist for screen reactions; missing or unreadable = nothing is captured.
    #[serde(default)]
    pub capture_blocklist: Option<PathBuf>,
    /// While you stay on one window, look again this often (seconds).
    #[serde(default = "default_sample")]
    pub screen_sample_s: u64,
    /// Her body: the Godot overlay felt-tray launches and drives. No `[body]` table = no body.
    #[serde(default)]
    pub body: Option<Body>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    /// Starts the overlay, e.g. `["godot", "--display-driver", "wayland", "--path", "/path/to/felt-godot-overlay"]`
    /// or an exported overlay binary. The relay port and secret flag are added after `--`.
    pub command: Vec<String>,
    /// Loopback port the overlay's relay listens on.
    #[serde(default = "default_body_port")]
    pub port: u16,
    /// The overlay's animation library (one folder per tag), e.g. `.../felt-godot-overlay/animations/library`.
    pub animations: PathBuf,
    /// Her body's data files (face poses, reactions, games...).
    #[serde(default = "default_body_data")]
    pub data_dir: PathBuf,
    /// Rhubarb Lip Sync (times her mouth to her voice, offline).
    #[serde(default = "default_rhubarb")]
    pub rhubarb: Vec<String>,
    /// Her recorded clips (`presence/zip_it_1.wav` ...); none = she has no canned lines.
    #[serde(default)]
    pub sounds: Option<PathBuf>,
    /// Captures what the speakers play (16 kHz mono f32 on stdout) so she can bob to music; [] = off.
    #[serde(default = "default_music_capture")]
    pub music_capture: Vec<String>,
}

fn default_music_capture() -> Vec<String> {
    [
        "pw-record",
        "--raw",
        "-P",
        "{ stream.capture.sink=true media.name=\"Felt listening for a beat\" }",
        "--rate",
        "16000",
        "--channels",
        "1",
        "--format",
        "f32",
        "-",
    ]
    .map(String::from)
    .to_vec()
}

fn default_body_port() -> u16 {
    8765
}

fn default_body_data() -> PathBuf {
    "/usr/share/felt-tray/body".into()
}

fn default_rhubarb() -> Vec<String> {
    ["rhubarb", "--recognizer", "phonetic"]
        .map(String::from)
        .to_vec()
}

fn default_agenda_days() -> u32 {
    14
}

fn default_sample() -> u64 {
    45
}

impl Config {
    pub fn blocklist_path(&self) -> Result<PathBuf> {
        match &self.capture_blocklist {
            Some(p) => Ok(p.clone()),
            None => Ok(path()?.with_file_name("capture_blocklist.toml")),
        }
    }
}

fn default_window_size() -> [f64; 2] {
    [800.0, 600.0]
}

fn default_player() -> Vec<String> {
    ["pw-play", "--media-role", "Communication", "-"]
        .map(String::from)
        .to_vec()
}

fn default_true() -> bool {
    true
}

fn default_session() -> String {
    "dashboard".into()
}

pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("FELT_TRAY_CONFIG") {
        return Ok(p.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(base.join("felt-tray/config.toml"))
}

pub fn data_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .context("neither XDG_DATA_HOME nor HOME is set")?;
    Ok(base.join("felt-tray"))
}

/// The commented template `felt-tray --init` writes.
pub const TEMPLATE: &str = include_str!("../config.example.toml");

pub enum Loaded {
    Ok(Box<Config>),
    /// No config file yet (first run).
    Missing(PathBuf),
}

pub fn load() -> Result<Loaded> {
    let p = path()?;
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Loaded::Missing(p)),
        Err(e) => return Err(e).with_context(|| format!("can't read {}", p.display())),
    };
    let c = parse(&text).with_context(|| format!("bad config in {}", p.display()))?;
    Ok(Loaded::Ok(Box::new(c)))
}

/// Writes the template unless a config already exists; returns its path.
pub fn init() -> Result<PathBuf> {
    let p = path()?;
    if p.exists() {
        anyhow::bail!("{} already exists - not touching it", p.display());
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, TEMPLATE)?;
    Ok(p)
}

fn parse(text: &str) -> Result<Config> {
    let mut c: Config = toml::from_str(text)?;
    c.felt_core_url = c.felt_core_url.trim_end_matches('/').to_owned();
    for (key, url) in [("felt_core_url", &c.felt_core_url), ("app_url", &c.app_url)] {
        anyhow::ensure!(
            url.starts_with("https://"),
            "{key} must be an https:// URL (it is {url:?})"
        );
    }
    anyhow::ensure!(
        !c.client_bundle.as_os_str().is_empty(),
        "client_bundle is empty"
    );
    anyhow::ensure!(!c.window_hosts.is_empty(), "window_hosts is empty");
    anyhow::ensure!(!c.player.is_empty(), "player is empty");
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_trims_the_core_url() {
        let c = parse(
            r#"
            felt_core_url = "https://core.test/"
            app_url = "https://app.test/app/"
            client_bundle = "/x/bundle.pem"
            window_hosts = ["app.test", "sso.test"]
            "#,
        )
        .unwrap();
        assert_eq!(c.felt_core_url, "https://core.test");
        assert_eq!(c.session_id, "dashboard");
        assert!(c.tray_icon);
    }

    #[test]
    fn refuses_unknown_keys() {
        let err = parse(
            r#"
            felt_core_url = "https://a"
            app_url = "https://b"
            client_bundle = "c"
            window_hosts = ["b"]
            felt_core_token = "nope"
            "#,
        );
        assert!(err.is_err());
    }

    #[test]
    fn the_template_refuses_to_start_until_filled() {
        let err = parse(TEMPLATE).unwrap_err().to_string();
        assert!(err.contains("felt_core_url"), "{err}");
    }

    #[test]
    fn plain_http_is_refused() {
        let err = parse(
            r#"
            felt_core_url = "http://core.test"
            app_url = "https://app.test/app/"
            client_bundle = "/x/bundle.pem"
            window_hosts = ["app.test"]
            "#,
        );
        assert!(err.is_err());
    }
}
