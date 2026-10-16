//! signals.games (WP-17, automatic detection): every decision is pure and
//! tested with synthetic inputs - classification, ignore patterns, title
//! cleanup, detect_games' hysteresis, the path-keyed tracker, games.toml and
//! libraryfolders.vdf parsing. No real process/registry dependency.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;

use felt_games::{
    GameDetector, GameSources, GameTracker, GamesConfig, ProcSample, build_sources,
    classify_process, clean_title, default_games_path, detect_games, diff_running_games,
    is_ignored_exe, load_games, load_games_config, norm_path, parse_libraryfolders_vdf,
};

const STEAM: &str = r"F:\SteamLibrary\steamapps\common";

static SOURCES: LazyLock<GameSources> = LazyLock::new(|| {
    build_sources(
        &GamesConfig {
            games: HashSet::from(["endfield.exe".to_string()]),
            roots: vec![r"F:\Games".to_string()],
            ..Default::default()
        },
        &[r"F:\SteamLibrary"],
        &[],
        &[],
        &[r"D:\Other\ffxiv_dx11.exe"],
        &[r"E:\Felt"],
    )
});

/// Python's `game(path, has_window=True, name=None, sources=SOURCES)` helper.
fn game(path: &str, has_window: bool, sources: &GameSources) -> bool {
    let name = path.rsplit('\\').next().unwrap_or(path);
    classify_process(Some(path), Some(name), has_window, sources)
}

fn keys(names: &[&str]) -> HashSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn events(started: &[&str], stopped: &[(&str, f64)]) -> (Vec<String>, Vec<(String, f64)>) {
    (
        started.iter().map(|s| (*s).to_string()).collect(),
        stopped
            .iter()
            .map(|(n, d)| ((*n).to_string(), *d))
            .collect(),
    )
}

fn temp_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("felt-games-{}-{}", test_name, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_games_toml(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("games.toml");
    std::fs::write(&path, contents).unwrap();
    path
}

// -- classify_process --------------------------------------------------------

#[test]
fn test_exe_under_a_steam_library_is_a_game() {
    let path = format!("{STEAM}\\Monster Hunter World\\MonsterHunterWorld.exe");
    assert!(game(&path, true, &SOURCES));
}

#[test]
fn test_exe_under_a_toml_root_is_a_game() {
    assert!(game(r"F:\Games\XIV\game\ffxiv_dx11.exe", true, &SOURCES));
}

#[test]
fn test_root_match_is_case_and_slash_insensitive() {
    assert!(game(
        "f:/steamlibrary/STEAMAPPS/common/Factorio/bin/x64/factorio.exe",
        true,
        &SOURCES
    ));
}

#[test]
fn test_root_prefix_must_be_a_whole_folder() {
    assert!(!game(r"F:\GamesBackup\thing.exe", true, &SOURCES));
}

#[test]
fn test_game_config_store_exe_is_a_game_anywhere() {
    assert!(game(r"D:\Other\ffxiv_dx11.exe", true, &SOURCES));
    assert!(!game(r"D:\Other\notagame.exe", true, &SOURCES));
}

#[test]
fn test_manual_name_is_a_game_anywhere_even_without_exe_path() {
    assert!(game(r"C:\Somewhere\Endfield.exe", true, &SOURCES));
    assert!(classify_process(None, Some("endfield.exe"), true, &SOURCES));
}

#[test]
fn test_no_exe_path_and_not_manual_is_not_a_game() {
    assert!(!classify_process(
        None,
        Some("MonsterHunterWorld.exe"),
        true,
        &SOURCES
    ));
}

#[test]
fn test_no_visible_window_is_never_a_game() {
    let path = format!("{STEAM}\\Monster Hunter World\\MonsterHunterWorld.exe");
    assert!(!game(&path, false, &SOURCES));
}

#[test]
fn test_random_windowed_app_is_not_a_game() {
    assert!(!game(
        r"C:\Program Files\Notepad++\notepad++.exe",
        true,
        &SOURCES
    ));
}

#[test]
fn test_wallpaper_engine_in_steamapps_is_not_a_game() {
    let a = format!("{STEAM}\\wallpaper_engine\\wallpaper64.exe");
    let b = format!("{STEAM}\\wallpaper_engine\\bin\\webwallpaper32.exe");
    assert!(!game(&a, true, &SOURCES));
    assert!(!game(&b, true, &SOURCES));
}

#[test]
fn test_launchers_and_helpers_under_roots_are_not_games() {
    for exe in [
        r"F:\Games\GRYPHLINK\Launcher.exe".to_string(),
        r"F:\Games\XIV\boot\ffxivboot64.exe".to_string(),
        format!("{STEAM}\\Aniimo\\UnityCrashHandler64.exe"),
        format!("{STEAM}\\Some Game\\EasyAntiCheat\\EasyAntiCheat_EOS.exe"),
    ] {
        assert!(!game(&exe, true, &SOURCES), "{exe}");
    }
}

#[test]
fn test_felt_itself_is_never_a_game() {
    assert!(!game(
        r"E:\Felt\felt-character-assets\felt-godot-overlay\build\windows\FeltOverlay.exe",
        true,
        &SOURCES
    ));
    let sources = build_sources(
        &GamesConfig {
            roots: vec![r"E:\\".to_string()],
            ..Default::default()
        },
        &[],
        &[],
        &[],
        &[],
        &[r"E:\Felt"],
    );
    assert!(!game(
        r"E:\Felt\felt-animation\.venv\Scripts\python.exe",
        true,
        &sources
    ));
}

#[test]
fn test_browsers_and_discord_are_never_games_even_if_listed() {
    let sources = build_sources(
        &GamesConfig {
            games: HashSet::from(["discord.exe".to_string()]),
            ..Default::default()
        },
        &[],
        &[],
        &[],
        &[r"C:\B\chrome.exe"],
        &[],
    );
    assert!(!game(
        r"C:\Users\x\AppData\Local\Discord\app-1\Discord.exe",
        true,
        &sources
    ));
    assert!(!game(r"C:\B\chrome.exe", true, &sources));
}

#[test]
fn test_manual_game_beats_default_ignore_pattern() {
    let sources = build_sources(
        &GamesConfig {
            games: HashSet::from(["gamelauncher.exe".to_string()]),
            ..Default::default()
        },
        &[],
        &[],
        &[],
        &[],
        &[],
    );
    assert!(game(r"C:\X\GameLauncher.exe", true, &sources));
}

// -- is_ignored_exe ------------------------------------------------------------

#[test]
fn test_default_ignore_names_and_patterns() {
    let sources = GameSources::default();
    for name in [
        "steam.exe",
        "steamwebhelper.exe",
        "gameoverlayui.exe",
        "epicgameslauncher.exe",
        "galaxyclient.exe",
        "UnityCrashHandler32.exe",
        "CrashReportClient.exe",
        "BEService.exe",
        "vrmonitor.exe",
        "wallpaper32.exe",
        "MyGameCrashpad.exe",
        "SomeLauncher.exe",
        "auto_updater.exe",
        "setup.exe",
        "UnInstall.exe",
        "vc_redist.x64.exe",
        "DXSETUP.exe",
        "unins000.exe",
    ] {
        assert!(is_ignored_exe(name, &sources), "{name}");
    }
}

#[test]
fn test_real_game_names_are_not_ignored() {
    let sources = GameSources::default();
    for name in [
        "MonsterHunterWorld.exe",
        "ffxiv_dx11.exe",
        "PathOfExile_x64Steam.exe",
        "factorio.exe",
        "Endfield.exe",
        "AC2-Win64-Shipping.exe",
        "Aniimo.exe",
    ] {
        assert!(!is_ignored_exe(name, &sources), "{name}");
    }
}

#[test]
fn test_toml_ignore_exact_and_glob() {
    let sources = build_sources(
        &GamesConfig {
            ignore_names: HashSet::from(["games.exe".to_string()]),
            ignore_patterns: vec!["*tool*".to_string()],
            ..Default::default()
        },
        &[],
        &[],
        &[],
        &[],
        &[],
    );
    assert!(is_ignored_exe("Games.exe", &sources));
    assert!(is_ignored_exe("moddingtool64.exe", &sources));
    assert!(!is_ignored_exe("factorio.exe", &sources));
}

// -- clean_title -----------------------------------------------------------------

#[test]
fn test_clean_title_examples() {
    let cases = [
        ("MONSTER HUNTER: WORLD(421810)", "MONSTER HUNTER: WORLD"),
        ("Factorio 2.0.55", "Factorio"),
        ("Path of Exile 2", "Path of Exile 2"),
        ("FINAL FANTASY XIV", "FINAL FANTASY XIV"),
        ("Some Game - Steam", "Some Game"),
        ("Cool Game v1.4.2b", "Cool Game"),
        ("Cool Game (64-bit, DX11)", "Cool Game"),
        ("Unreal Thing [Build 12345] - Version 0.9", "Unreal Thing"),
        ("  Spaced    Out   Game  ", "Spaced Out Game"),
        ("Game (Remastered)", "Game (Remastered)"),
    ];
    for (raw, want) in cases {
        assert_eq!(clean_title(Some(raw), Some("x.exe")), want, "{raw}");
    }
}

#[test]
fn test_clean_title_falls_back_to_exe_stem() {
    assert_eq!(
        clean_title(Some(""), Some(r"F:\x\Endfield.exe")),
        "Endfield"
    );
    assert_eq!(clean_title(None, Some("AC2-Win64-Shipping.exe")), "AC2");
    assert_eq!(
        clean_title(Some("1.0.3"), Some(r"C:\g\factorio.exe")),
        "factorio"
    );
}

// -- detect_games -------------------------------------------------------------------

#[test]
fn test_detect_games_keys_by_path_and_names_by_title() {
    let mhw = format!("{STEAM}\\Monster Hunter World\\MonsterHunterWorld.exe");
    let samples = [
        ProcSample::new(
            1,
            "MonsterHunterWorld.exe",
            Some(&mhw),
            true,
            Some("MONSTER HUNTER: WORLD(421810)"),
        ),
        ProcSample::new(
            2,
            "Discord.exe",
            Some(r"C:\D\Discord.exe"),
            true,
            Some("Discord"),
        ),
        ProcSample::new(
            3,
            "steam.exe",
            Some(r"C:\Program Files (x86)\Steam\steam.exe"),
            true,
            Some("Steam"),
        ),
    ];
    let mut want = HashMap::new();
    want.insert(norm_path(&mhw), "MONSTER HUNTER: WORLD".to_string());
    assert_eq!(detect_games(&samples, &SOURCES, &HashSet::new()), want);
}

#[test]
fn test_detect_games_one_entry_per_exe_path() {
    let path = format!("{STEAM}\\A\\a.exe");
    let samples = [
        ProcSample::new(1, "a.exe", Some(&path), true, Some("A")),
        ProcSample::new(2, "a.exe", Some(&path), true, Some("A2")),
    ];
    assert_eq!(detect_games(&samples, &SOURCES, &HashSet::new()).len(), 1);
}

#[test]
fn test_detect_games_keeps_tracked_game_without_window() {
    let mhw = format!("{STEAM}\\Monster Hunter World\\MonsterHunterWorld.exe");
    let samples = [ProcSample::new(
        1,
        "MonsterHunterWorld.exe",
        Some(&mhw),
        false,
        None,
    )];
    assert!(detect_games(&samples, &SOURCES, &HashSet::new()).is_empty());
    let key = norm_path(&mhw);
    let tracked = keys(&[key.as_str()]);
    assert!(detect_games(&samples, &SOURCES, &tracked).contains_key(&key));
}

#[test]
fn test_detect_games_manual_name_without_exe_path() {
    let got = detect_games(
        &[ProcSample::new(
            9,
            "Endfield.exe",
            None,
            true,
            Some("Endfield"),
        )],
        &SOURCES,
        &HashSet::new(),
    );
    assert_eq!(got, map(&[("name:endfield.exe", "Endfield")]));
}

// -- GameTracker / diff ---------------------------------------------------------------

#[test]
fn test_diff_running_games() {
    assert_eq!(
        diff_running_games(&HashSet::new(), &keys(&["k"])),
        (keys(&["k"]), HashSet::new())
    );
    assert_eq!(
        diff_running_games(&keys(&["k"]), &HashSet::new()),
        (HashSet::new(), keys(&["k"]))
    );
    assert_eq!(
        diff_running_games(&keys(&["k"]), &keys(&["k"])),
        (HashSet::new(), HashSet::new())
    );
}

#[test]
fn test_tracker_start_and_stop_with_duration_and_start_name() {
    let clock = Cell::new(1000.0_f64);
    let mut tracker = GameTracker::new(|| clock.get());
    assert_eq!(
        tracker.update(&map(&[("c:\\g\\a.exe", "Game A")])),
        events(&["Game A"], &[])
    );
    assert_eq!(tracker.running_names(), ["Game A"]);
    clock.set(clock.get() + 90.0);
    assert_eq!(
        tracker.update(&HashMap::new()),
        events(&[], &[("Game A", 90.0)])
    );
}

#[test]
fn test_tracker_title_change_is_not_a_stop_start() {
    let clock = Cell::new(0.0_f64);
    let mut tracker = GameTracker::new(|| clock.get());
    tracker.update(&map(&[("c:\\g\\a.exe", "Loading...")]));
    clock.set(30.0);
    assert_eq!(
        tracker.update(&map(&[("c:\\g\\a.exe", "Game A - Main Menu")])),
        events(&[], &[])
    );
    clock.set(60.0);
    // reported under its start name
    assert_eq!(
        tracker.update(&HashMap::new()),
        events(&[], &[("Loading...", 60.0)])
    );
}

#[test]
fn test_tracker_handles_two_games_independently() {
    let clock = Cell::new(0.0_f64);
    let mut tracker = GameTracker::new(|| clock.get());
    tracker.update(&map(&[("a", "A")]));
    clock.set(10.0);
    assert_eq!(
        tracker.update(&map(&[("a", "A"), ("b", "B")])),
        events(&["B"], &[])
    );
    clock.set(40.0);
    assert_eq!(
        tracker.update(&map(&[("b", "B")])),
        events(&[], &[("A", 40.0)])
    );
    assert_eq!(tracker.running_keys(), keys(&["b"]));
}

// -- GameDetector source caching ---------------------------------------------------------

#[test]
fn test_detector_refreshes_sources_on_interval() {
    let clock = Rc::new(Cell::new(0.0_f64));
    let calls = Rc::new(RefCell::new(Vec::new()));
    let clock_now = Rc::clone(&clock);
    let clock_discover = Rc::clone(&clock);
    let calls_discover = Rc::clone(&calls);
    let mut detector = GameDetector::new(
        600.0,
        move || clock_now.get(),
        move |_: &Path| {
            calls_discover.borrow_mut().push(clock_discover.get());
            GameSources::default()
        },
    );
    detector.sources();
    clock.set(300.0);
    detector.sources();
    clock.set(601.0);
    detector.sources();
    assert_eq!(*calls.borrow(), [0.0, 601.0]);
}

// -- games.toml / libraryfolders.vdf ---------------------------------------------------------

#[test]
fn test_load_games_config_parses_all_sections() {
    let dir = temp_dir("parses");
    let path = write_games_toml(
        &dir,
        "games = [\"Endfield.exe\"]\nignore = [\"Games.exe\", \"*Tool*\"]\nroots = ['F:\\Games']\n",
    );
    let config = load_games_config(&path);
    assert_eq!(config.games, keys(&["endfield.exe"]));
    assert_eq!(config.ignore_names, keys(&["games.exe"]));
    assert_eq!(config.ignore_patterns, ["*tool*"]);
    assert_eq!(config.roots, ["F:\\Games"]);
    assert_eq!(load_games(&path), keys(&["endfield.exe"]));
}

#[test]
fn test_load_games_config_missing_or_malformed_is_empty() {
    let dir = temp_dir("malformed");
    assert_eq!(
        load_games_config(&dir.join("nope.toml")),
        GamesConfig::default()
    );
    let bad = write_games_toml(&dir, "not valid [[[ toml");
    assert_eq!(load_games_config(&bad), GamesConfig::default());
}

#[test]
fn test_shipped_games_toml_keeps_endfield() {
    assert!(load_games(&default_games_path()).contains("endfield.exe"));
}

#[test]
fn test_parse_libraryfolders_vdf() {
    let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"apps\" { \"228980\" \"1\" }\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"F:\\\\SteamLibrary\"\n\t}\n}";
    assert_eq!(
        parse_libraryfolders_vdf(text),
        ["C:\\Program Files (x86)\\Steam", "F:\\SteamLibrary"]
    );
}

#[test]
fn test_build_sources_records_origins() {
    let sources = build_sources(
        &GamesConfig {
            roots: vec![r"F:\Games".to_string()],
            ..Default::default()
        },
        &[r"F:\SteamLibrary"],
        &[r"D:\Epic\Fortnite"],
        &[r"D:\GOG\Witcher 3"],
        &[],
        &[],
    );
    let mut want = HashMap::new();
    want.insert(norm_path(STEAM), "steam".to_string());
    want.insert(norm_path(r"D:\Epic\Fortnite"), "epic".to_string());
    want.insert(norm_path(r"D:\GOG\Witcher 3"), "gog".to_string());
    want.insert(norm_path(r"F:\Games"), "games.toml".to_string());
    assert_eq!(sources.root_origins, want);
}
