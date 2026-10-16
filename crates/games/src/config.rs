//! games.toml parsing and game-library source assembly.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::classify::{DEFAULT_IGNORE_NAMES, DEFAULT_IGNORE_PATTERNS, GameSources};
use crate::paths::{norm_path, ntpath_join};

/// Manual extras from games.toml.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GamesConfig {
    pub games: HashSet<String>,
    pub ignore_names: HashSet<String>,
    pub ignore_patterns: Vec<String>,
    pub roots: Vec<String>,
}

/// games.toml's manual extras. Missing/malformed file -> empty config
/// (auto-detection still works), never a crash.
pub fn load_games_config(path: &Path) -> GamesConfig {
    let Ok(text) = fs::read_to_string(path) else {
        return GamesConfig::default();
    };
    let Ok(data) = toml::from_str::<toml::Table>(&text) else {
        return GamesConfig::default();
    };
    let names = |key: &str| -> Vec<String> {
        data.get(key)
            .and_then(toml::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(toml_display)
                    .map(|n| n.trim().to_lowercase())
                    .filter(|n| !n.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let ignore = names("ignore");
    GamesConfig {
        games: names("games").into_iter().collect(),
        ignore_names: ignore
            .iter()
            .filter(|n| !n.contains(['*', '?', '[']))
            .cloned()
            .collect(),
        ignore_patterns: ignore
            .into_iter()
            .filter(|n| n.contains(['*', '?', '[']))
            .collect(),
        roots: data
            .get("roots")
            .and_then(toml::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(toml_display)
                    .map(|r| r.trim().to_string())
                    .filter(|r| !r.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Back-compat: just the manual `games` names.
pub fn load_games(path: &Path) -> HashSet<String> {
    load_games_config(path).games
}

static VDF_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)"path"\s+"([^"]+)""#).unwrap());

/// Pure: every library `"path"` in Steam's libraryfolders.vdf.
pub fn parse_libraryfolders_vdf(text: &str) -> Vec<String> {
    VDF_PATH
        .captures_iter(text)
        .map(|m| m[1].replace(r"\\", r"\"))
        .collect()
}

/// Pure: assemble [`GameSources`] from the discovered pieces.
pub fn build_sources(
    config: &GamesConfig,
    steam_libraries: &[&str],
    epic_locations: &[&str],
    gog_paths: &[&str],
    known_exes: &[&str],
    exclude_roots: &[&str],
) -> GameSources {
    // root -> origin, first claim wins; insertion order is the roots order.
    let mut origins: Vec<(String, String)> = Vec::new();
    let mut setdefault = |key: String, origin: &str| {
        if !origins.iter().any(|(k, _)| *k == key) {
            origins.push((key, origin.to_string()));
        }
    };
    for lib in steam_libraries {
        setdefault(
            norm_path(&ntpath_join(lib, &["steamapps", "common"])),
            "steam",
        );
    }
    for loc in epic_locations {
        setdefault(norm_path(loc), "epic");
    }
    for loc in gog_paths {
        setdefault(norm_path(loc), "gog");
    }
    for loc in &config.roots {
        setdefault(norm_path(loc), "games.toml");
    }
    let mut ignore_names: HashSet<String> = DEFAULT_IGNORE_NAMES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    ignore_names.extend(config.ignore_names.iter().cloned());
    let mut ignore_patterns: Vec<String> = DEFAULT_IGNORE_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    ignore_patterns.extend(config.ignore_patterns.iter().cloned());
    GameSources {
        roots: origins.iter().map(|(k, _)| k.clone()).collect(),
        known_exes: known_exes
            .iter()
            .filter(|p| !p.is_empty())
            .map(|p| norm_path(p))
            .collect(),
        manual_names: config.games.clone(),
        ignore_names,
        ignore_patterns,
        exclude_roots: exclude_roots.iter().map(|r| norm_path(r)).collect(),
        root_origins: origins.into_iter().collect(),
    }
}

/// Python `str(n)` for the scalar values games.toml lists may hold.
fn toml_display(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        toml::Value::Datetime(d) => Some(d.to_string()),
        _ => None,
    }
}
