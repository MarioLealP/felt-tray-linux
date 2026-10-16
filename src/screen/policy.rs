//! What may be captured - the privacy guarantee, decided over the window LIST (app ids, titles), never over
//! pixels. Port of the Windows tray's capture_policy (WP-07) for KWin windows: fail closed (an unidentified
//! window is blocked), Felt never captures herself, and the decision line never logs a rejected title.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

/// One window as KWin reports it (our KWin script).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WindowInfo {
    /// KWin's internal id, `{uuid}` - what ScreenShot2.CaptureWindow takes.
    pub id: String,
    /// resourceClass ("firefox", "equibop", "org.keepassxc.KeePassXC"); empty = unidentified.
    #[serde(default)]
    pub app: String,
    /// desktopFileName, when the app sets one.
    #[serde(default)]
    pub desktop_file: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pid: u32,
    /// Top-left corner, desktop px.
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub w: u32,
    #[serde(default)]
    pub h: u32,
    #[serde(default)]
    pub fullscreen: bool,
}

impl WindowInfo {
    fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    /// For logs: the app id only.
    pub fn label(&self) -> &str {
        if self.app.is_empty() { "?" } else { &self.app }
    }
}

#[derive(Debug, Default)]
pub struct Blocklist {
    /// lowercased app ids, matched against resourceClass and desktopFileName
    apps: Vec<String>,
    titles: Vec<Regex>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlocklistFile {
    #[serde(default)]
    apps: Vec<String>,
    #[serde(default)]
    title_regexes: Vec<String>,
}

impl Blocklist {
    pub fn parse(text: &str) -> Result<Self> {
        let f: BlocklistFile = toml::from_str(text)?;
        let titles = f
            .title_regexes
            .iter()
            .map(|r| Regex::new(r).with_context(|| format!("bad title regex {r:?}")))
            .collect::<Result<_>>()?;
        Ok(Self {
            apps: f.apps.iter().map(|a| a.to_lowercase()).collect(),
            titles,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("can't read the capture blocklist {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("bad capture blocklist {}", path.display()))
    }
}

/// Why `w` is blocked by the privacy policy, or None if allowed.
pub fn block_reason(w: &WindowInfo, bl: &Blocklist) -> Option<&'static str> {
    if w.app.trim().is_empty() {
        return Some("unidentified");
    }
    let ids = [w.app.to_lowercase(), w.desktop_file.to_lowercase()];
    if ids.iter().any(|id| !id.is_empty() && bl.apps.contains(id)) {
        return Some("app");
    }
    if bl.titles.iter().any(|r| r.is_match(&w.title)) {
        return Some("title");
    }
    None
}

/// Felt's own window titles: the chat window ("Felt", exact) and her Godot windows, which start with one of
/// these ("Felt Overlay", "Felt Overlay (screen 2)", "Felt Studio").
const FELT_TITLE_PREFIXES: [&str; 2] = ["Felt Overlay", "Felt Studio"];

fn is_felt_title(title: &str) -> bool {
    title == "Felt" || FELT_TITLE_PREFIXES.iter().any(|p| title.starts_with(p))
}

/// Felt never captures herself: our own process, or one of her window titles.
pub fn exclusion_reason(w: &WindowInfo, own_pid: u32) -> Option<&'static str> {
    if w.pid != 0 && w.pid == own_pid {
        return Some("felt-pid");
    }
    if is_felt_title(&w.title) {
        return Some("felt-title");
    }
    None
}

#[derive(Debug, Default)]
pub struct Decision<'a> {
    pub target: Option<&'a WindowInfo>,
    pub rejected: Vec<(&'a WindowInfo, &'static str)>,
}

/// The active window if allowed; else the largest allowed fullscreen window; else the largest allowed window;
/// else nothing (no capture, no reaction).
pub fn decide<'a>(
    windows: &'a [WindowInfo],
    bl: &Blocklist,
    active: Option<&str>,
    own_pid: u32,
) -> Decision<'a> {
    let mut d = Decision::default();
    let mut allowed = Vec::new();
    for w in windows {
        match exclusion_reason(w, own_pid).or_else(|| block_reason(w, bl)) {
            Some(r) => d.rejected.push((w, r)),
            None => allowed.push(w),
        }
    }
    d.target = active
        .and_then(|id| allowed.iter().copied().find(|w| w.id == id))
        .or_else(|| {
            let full: Vec<_> = allowed.iter().copied().filter(|w| w.fullscreen).collect();
            let pool = if full.is_empty() { &allowed } else { &full };
            pool.iter().copied().max_by_key(|w| w.area())
        });
    d
}

/// One compact line: target + rejected windows grouped by app:reason. Rejected titles are never logged.
pub fn format_decision(d: &Decision<'_>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (w, r) in &d.rejected {
        *counts.entry(format!("{}:{r}", w.label())).or_default() += 1;
    }
    let rejected: Vec<String> = counts
        .into_iter()
        .map(|(k, n)| if n > 1 { format!("{k}x{n}") } else { k })
        .collect();
    let target = d.target.map_or("none", |w| w.label());
    format!("target={target} rejected=[{}]", rejected.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: &str, app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: id.into(),
            app: app.into(),
            title: title.into(),
            w: 800,
            h: 600,
            ..Default::default()
        }
    }

    fn bl(apps: &[&str], titles: &[&str]) -> Blocklist {
        Blocklist {
            apps: apps.iter().map(|a| a.to_lowercase()).collect(),
            titles: titles.iter().map(|t| Regex::new(t).unwrap()).collect(),
        }
    }

    #[test]
    fn allowed_app_is_not_blocked() {
        assert_eq!(
            block_reason(&win("1", "kate", "x"), &bl(&["equibop"], &[])),
            None
        );
    }

    #[test]
    fn blocked_app_matches_case_insensitively() {
        assert_eq!(
            block_reason(&win("1", "Equibop", "x"), &bl(&["equibop"], &[])),
            Some("app")
        );
    }

    #[test]
    fn blocked_by_desktop_file_name_too() {
        let mut w = win("1", "electron", "x");
        w.desktop_file = "vesktop".into();
        assert_eq!(block_reason(&w, &bl(&["vesktop"], &[])), Some("app"));
    }

    #[test]
    fn unidentified_window_is_blocked_fail_closed() {
        assert_eq!(
            block_reason(&win("1", "", "x"), &Blocklist::default()),
            Some("unidentified")
        );
        assert_eq!(
            block_reason(&win("1", "  ", "x"), &Blocklist::default()),
            Some("unidentified")
        );
    }

    #[test]
    fn blocked_title_regex() {
        let b = bl(&[], &[r"(?i)\bbank\b"]);
        assert_eq!(
            block_reason(&win("1", "firefox", "My Bank - Firefox"), &b),
            Some("title")
        );
        assert_eq!(
            block_reason(&win("1", "firefox", "Bankruptcy wiki"), &b),
            None
        );
    }

    #[test]
    fn active_allowed_window_wins() {
        let ws = [win("a", "kate", "a"), win("b", "firefox", "b")];
        assert_eq!(
            decide(&ws, &bl(&[], &[]), Some("a"), 1).target.unwrap().id,
            "a"
        );
    }

    #[test]
    fn active_blocked_window_falls_through_to_largest_allowed() {
        let mut big = win("b", "firefox", "b");
        big.w = 2000;
        let ws = [win("a", "equibop", "a"), win("s", "kate", "s"), big];
        assert_eq!(
            decide(&ws, &bl(&["equibop"], &[]), Some("a"), 1)
                .target
                .unwrap()
                .id,
            "b"
        );
    }

    #[test]
    fn fullscreen_allowed_window_preferred_over_larger_windowed() {
        let mut game = win("g", "ffxiv_dx11.exe", "FFXIV");
        game.fullscreen = true;
        let mut big = win("b", "firefox", "b");
        big.w = 4000;
        let ws = [big, game];
        assert_eq!(decide(&ws, &bl(&[], &[]), None, 1).target.unwrap().id, "g");
    }

    #[test]
    fn all_blocked_or_empty_gives_no_target() {
        let ws = [win("a", "equibop", "a")];
        assert!(
            decide(&ws, &bl(&["equibop"], &[]), Some("a"), 1)
                .target
                .is_none()
        );
        assert!(decide(&[], &bl(&[], &[]), None, 1).target.is_none());
    }

    #[test]
    fn felt_never_captures_her_own_windows() {
        let mut chat = win("c", "felt-tray", "Felt");
        chat.pid = 42;
        assert!(
            decide(std::slice::from_ref(&chat), &bl(&[], &[]), Some("c"), 42)
                .target
                .is_none()
        );
        chat.pid = 0; // no pid info: the title still excludes it
        assert!(
            decide(std::slice::from_ref(&chat), &bl(&[], &[]), Some("c"), 42)
                .target
                .is_none()
        );
    }

    #[test]
    fn her_overlay_side_windows_are_excluded() {
        for t in [
            "Felt Overlay",
            "Felt Overlay (screen 2)",
            "Felt Overlay (DEBUG)",
            "Felt Studio",
        ] {
            assert_eq!(
                exclusion_reason(&win("o", "godot", t), 1),
                Some("felt-title")
            );
        }
        assert_eq!(exclusion_reason(&win("o", "kate", "Felt notes"), 1), None);
    }

    #[test]
    fn felt_in_a_title_is_not_enough_to_exclude_other_apps() {
        let ws = [win("k", "kate", "notes about Felt - Kate")];
        assert!(decide(&ws, &bl(&[], &[]), Some("k"), 1).target.is_some());
    }

    #[test]
    fn decision_line_is_compact_and_titleless() {
        let mut chat = win("c", "felt-tray", "Felt");
        chat.pid = 7;
        let ws = [
            win("1", "equibop", "#secret-channel - Discord"),
            win("2", "org.keepassxc.KeePassXC", "Passwords"),
            win("3", "org.keepassxc.KeePassXC", "Passwords 2"),
            chat,
            win("5", "kate", "Untitled"),
        ];
        let b = bl(&["equibop", "org.keepassxc.KeePassXC"], &[]);
        let line = format_decision(&decide(&ws, &b, Some("5"), 7));
        assert_eq!(
            line,
            "target=kate rejected=[equibop:app, felt-tray:felt-pid, org.keepassxc.KeePassXC:appx2]"
        );
        assert!(!line.contains("secret"));
    }

    #[test]
    fn bad_regex_is_an_error_not_a_silent_allow() {
        assert!(Blocklist::parse(r#"title_regexes = ["(unclosed"]"#).is_err());
        assert!(Blocklist::parse(r#"processes = ["discord.exe"]"#).is_err()); // Windows keys refused
    }
}

#[cfg(test)]
mod example_blocklist {
    use super::*;

    fn example() -> Blocklist {
        Blocklist::parse(include_str!("../../capture_blocklist.example.toml"))
            .expect("example parses")
    }

    fn win(app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: "1".into(),
            app: app.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn covers_every_required_app() {
        let b = example();
        for app in [
            "equibop",
            "discord",
            "vesktop",
            "legcord",
            "telegram-desktop",
            "org.telegram.desktop",
            "whatsapp-for-linux",
            "signal-desktop",
            "org.keepassxc.KeePassXC",
            "keepassxc",
            "kdeconnect.sms",
        ] {
            assert_eq!(
                block_reason(&win(app, "x"), &b),
                Some("app"),
                "{app} must be blocked"
            );
        }
    }

    #[test]
    fn allows_browsers_and_files_but_not_banks_or_private_windows() {
        let b = example();
        for (app, title) in [
            ("firefox", "Path of Exile 2 wiki - Mozilla Firefox"),
            ("chromium", "GitHub - Chromium"),
            ("org.kde.dolphin", "Downloads - Dolphin"),
        ] {
            assert_eq!(block_reason(&win(app, title), &b), None, "{title}");
        }
        for (app, title) in [
            ("firefox", "Millennium Bank - Online Banking"),
            ("firefox", "Mozilla Firefox Private Browsing"),
            ("firefox", "Nova aba — Navegação privada do Mozilla Firefox"),
            ("chromium", "New Tab - Chromium (Incognito)"),
            ("brave-browser", "New Tab - Brave (Private)"),
            ("firefox", "Tor Browser"),
        ] {
            assert_eq!(block_reason(&win(app, title), &b), Some("title"), "{title}");
        }
    }
}
