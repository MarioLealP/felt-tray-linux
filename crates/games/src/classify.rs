//! Process classification: is this process a game?

use std::collections::HashSet;

use crate::paths::{exe_name_of, fnmatch_case, is_under, norm_path};

/// Launchers, helpers, crash handlers, anti-cheat, overlays, VR runtime,
/// wallpaper engines, redistributables - exact exe names (lowercased).
pub const DEFAULT_IGNORE_NAMES: &[&str] = &[
    // Steam
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "gameoverlayui.exe",
    "gameoverlayui64.exe",
    "steamerrorreporter.exe",
    "steamerrorreporter64.exe",
    // Epic / GOG / other stores
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "epiconlineservices.exe",
    "epiconlineserviceshost.exe",
    "galaxyclient.exe",
    "galaxyclient helper.exe",
    "galaxycommunication.exe",
    "gog galaxy notifications renderer.exe",
    "eadesktop.exe",
    "eabackgroundservice.exe",
    "origin.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "uplaywebcore.exe",
    "battle.net.exe",
    "agent.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "riotclientuxrender.exe",
    // Crash handlers
    "unitycrashhandler64.exe",
    "unitycrashhandler32.exe",
    "crashreportclient.exe",
    "blizzarderror.exe",
    // Anti-cheat
    "easyanticheat.exe",
    "easyanticheat_eos.exe",
    "easyanticheat_setup.exe",
    "beservice.exe",
    "beservice_x64.exe",
    "battleye launcher.exe",
    "vgc.exe",
    "vgtray.exe",
    // VR runtime
    "vrserver.exe",
    "vrmonitor.exe",
    "vrcompositor.exe",
    "vrdashboard.exe",
    "vrstartup.exe",
    "vrwebhelper.exe",
    // Wallpaper Engine (lives in steamapps\common!)
    "wallpaper32.exe",
    "wallpaper64.exe",
    "webwallpaper32.exe",
    "ui32.exe",
    // Embedded web views / helper processes games and launchers spawn
    "qtwebengineprocess.exe",
    "cefviewwing.exe",
    "cefsharp.browsersubprocess.exe",
    "msedgewebview2.exe",
    "platformprocess.exe",
    "blizzardbrowser.exe",
    "7zg.exe",
    "patch.exe",
];

/// Glob patterns on the lowercased exe name.
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    "*crash*",
    "*launcher*",
    "*updater*",
    "*setup*",
    "*install*",
    "*redist*",
    "vc_redist*",
    "dxsetup*",
    "unins*",
    "*anticheat*",
    "ace-*",
    "*webhelper*",
    "*repair*",
    "*voiceproxy*",
    "*dumpanalyzer*",
    "*errorreporter*",
    "*overlay*",
    "*config*",
    "*sysinfo*",
    "*boot*",
];

/// Never games, mirroring felt-tray's privacy blocklist (browsers, Discord,
/// messengers, File Explorer, password managers) plus Felt's own processes.
pub const NEVER_GAMES: &[&str] = &[
    "discord.exe",
    "discordptb.exe",
    "discordcanary.exe",
    "discorddevelopment.exe",
    "vesktop.exe",
    "legcord.exe",
    "armcord.exe",
    "webcord.exe",
    "chrome.exe",
    "chromium.exe",
    "msedge.exe",
    "firefox.exe",
    "librewolf.exe",
    "floorp.exe",
    "waterfox.exe",
    "mullvadbrowser.exe",
    "palemoon.exe",
    "seamonkey.exe",
    "brave.exe",
    "opera.exe",
    "opera_gx.exe",
    "vivaldi.exe",
    "arc.exe",
    "zen.exe",
    "thorium.exe",
    "supermium.exe",
    "browser.exe",
    "yandex.exe",
    "whale.exe",
    "comet.exe",
    "duckduckgo.exe",
    "iexplore.exe",
    "whatsapp.exe",
    "whatsapp.root.exe",
    "phoneexperiencehost.exe",
    "telegram.exe",
    "keepassxc.exe",
    "keepass.exe",
    "explorer.exe",
    "feltoverlay.exe",
    "felt-tray.exe",
    "felt-vision.exe",
];

/// Everything [`classify_process`] decides against. Paths are [`norm_path`]'d.
#[derive(Clone, Debug)]
pub struct GameSources {
    pub roots: Vec<String>,
    /// GameConfigStore full exe paths.
    pub known_exes: HashSet<String>,
    /// games.toml `games`.
    pub manual_names: HashSet<String>,
    pub ignore_names: HashSet<String>,
    pub ignore_patterns: Vec<String>,
    /// Felt's own tree.
    pub exclude_roots: Vec<String>,
    /// root -> "steam"/"epic"/...
    pub root_origins: std::collections::HashMap<String, String>,
}

impl Default for GameSources {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            known_exes: HashSet::new(),
            manual_names: HashSet::new(),
            ignore_names: DEFAULT_IGNORE_NAMES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            ignore_patterns: DEFAULT_IGNORE_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            exclude_roots: Vec::new(),
            root_origins: std::collections::HashMap::new(),
        }
    }
}

// The Python dataclass marks `root_origins` compare=False: it is metadata,
// not part of identity.
impl PartialEq for GameSources {
    fn eq(&self, other: &Self) -> bool {
        self.roots == other.roots
            && self.known_exes == other.known_exes
            && self.manual_names == other.manual_names
            && self.ignore_names == other.ignore_names
            && self.ignore_patterns == other.ignore_patterns
            && self.exclude_roots == other.exclude_roots
    }
}

/// Launchers/helpers/tools, browsers/Discord, Felt, games.toml ignore.
pub fn is_ignored_exe(name: &str, sources: &GameSources) -> bool {
    let name = name.to_lowercase();
    if NEVER_GAMES.contains(&name.as_str()) || sources.ignore_names.contains(&name) {
        return true;
    }
    sources
        .ignore_patterns
        .iter()
        .any(|pattern| fnmatch_case(&name, pattern))
}

/// Pure: is this process a game? `exe_path` may be `None` (AccessDenied)
/// - then only the manual name list can match.
pub fn classify_process(
    exe_path: Option<&str>,
    name: Option<&str>,
    has_window: bool,
    sources: &GameSources,
) -> bool {
    if !has_window {
        return false;
    }
    let exe_path = exe_path.filter(|p| !p.is_empty());
    let name = match name.filter(|n| !n.is_empty()) {
        Some(n) => n.to_lowercase(),
        None => match exe_path {
            Some(p) => exe_name_of(p),
            None => String::new(),
        },
    };
    if name.is_empty() || NEVER_GAMES.contains(&name.as_str()) {
        return false;
    }
    let key = exe_path.map(norm_path);
    if let Some(key) = &key
        && sources.exclude_roots.iter().any(|root| is_under(key, root))
    {
        return false;
    }
    // An explicit game beats the ignore patterns.
    if sources.manual_names.contains(&name) {
        return true;
    }
    let Some(key) = key else {
        return false;
    };
    if is_ignored_exe(&name, sources) {
        return false;
    }
    sources.known_exes.contains(&key) || sources.roots.iter().any(|root| is_under(&key, root))
}
