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

pub fn load() -> Result<Config> {
    let p = path()?;
    let text = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "can't read {} - copy config.example.toml there",
            p.display()
        )
    })?;
    parse(&text).with_context(|| format!("bad config in {}", p.display()))
}

fn parse(text: &str) -> Result<Config> {
    let mut c: Config = toml::from_str(text)?;
    c.felt_core_url = c.felt_core_url.trim_end_matches('/').to_owned();
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
            felt_core_url = "a"
            app_url = "b"
            client_bundle = "c"
            window_hosts = []
            felt_core_token = "nope"
            "#,
        );
        assert!(err.is_err());
    }
}
