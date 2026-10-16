//! sounds.py (WP-19): tag->kind mapping and random file picking, no real
//! audio device involved. Ported 1:1 from
//! ref/felt-animation/tests/test_sounds.py.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use felt_sounds::{
    SeededRng, SoundKind, SoundMap, load_sound_map, pick_filler_path, pick_kind_sound,
    pick_presence_path, pick_sound_path,
};

/// Fresh directory per test, like pytest's tmp_path.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "felt_sounds_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        TmpDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Narrows the Option<PathBuf>-when-nothing-curated-yet return shape to
/// non-None for tests where a candidate file always exists.
fn must(path: Option<PathBuf>) -> PathBuf {
    assert!(path.is_some());
    path.expect("candidate path")
}

#[test]
fn test_load_sound_map_missing_file_uses_the_default_table() {
    let tmp = TmpDir::new();
    let result = load_sound_map(&tmp.path().join("does_not_exist.json"));
    assert_eq!(result["sleepy/yawn"], SoundKind::from("yawn"));
    assert_eq!(
        result["emotion/laughing"],
        SoundKind::from(["laugh", "giggle"])
    );
}

#[test]
fn test_load_sound_map_malformed_file_uses_the_default_table() {
    let tmp = TmpDir::new();
    let path = tmp.path().join("sounds.json");
    std::fs::write(&path, "not valid [[[ json").expect("write file");
    let result = load_sound_map(&path);
    assert_eq!(result["sleepy/yawn"], SoundKind::from("yawn"));
}

#[test]
fn test_load_sound_map_reads_a_real_file() {
    let tmp = TmpDir::new();
    let path = tmp.path().join("sounds.json");
    std::fs::write(&path, r#"{"gesture/wave": "greeting_sound"}"#).expect("write file");
    assert_eq!(
        load_sound_map(&path),
        SoundMap::from([("gesture/wave".to_owned(), SoundKind::from("greeting_sound"))])
    );
}

#[test]
fn test_load_sound_map_skips_comment_keys() {
    let tmp = TmpDir::new();
    let path = tmp.path().join("sounds.json");
    std::fs::write(
        &path,
        r#"{"_comment": "explaining this file", "sleepy/yawn": "yawn"}"#,
    )
    .expect("write file");
    assert_eq!(
        load_sound_map(&path),
        SoundMap::from([("sleepy/yawn".to_owned(), SoundKind::from("yawn"))])
    );
}

#[test]
fn test_pick_sound_path_none_when_kind_folder_missing() {
    let tmp = TmpDir::new();
    assert!(pick_sound_path("yawn", tmp.path(), None, None).is_none());
}

#[test]
fn test_pick_sound_path_none_when_kind_folder_empty() {
    let tmp = TmpDir::new();
    std::fs::create_dir(tmp.path().join("yawn")).expect("create dir");
    assert!(pick_sound_path("yawn", tmp.path(), None, None).is_none());
}

#[test]
fn test_pick_sound_path_only_considers_wav_files() {
    let tmp = TmpDir::new();
    let kind_dir = tmp.path().join("yawn");
    std::fs::create_dir(&kind_dir).expect("create dir");
    std::fs::write(kind_dir.join("take1.wav"), b"fake").expect("write file");
    std::fs::write(kind_dir.join("notes.txt"), b"not audio").expect("write file");
    let result = pick_sound_path("yawn", tmp.path(), None, None);
    assert!(result.is_some());
    assert_eq!(must(result).file_name().expect("file name"), "take1.wav");
}

#[test]
fn test_pick_sound_path_picks_randomly_among_takes() {
    let tmp = TmpDir::new();
    let kind_dir = tmp.path().join("yawn");
    std::fs::create_dir(&kind_dir).expect("create dir");
    for i in 0..5 {
        std::fs::write(kind_dir.join(format!("take{i}.wav")), b"fake").expect("write file");
    }
    let seen: HashSet<String> = (0..20)
        .map(|seed| {
            must(pick_sound_path(
                "yawn",
                tmp.path(),
                Some(&SeededRng::new(seed)),
                None,
            ))
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned()
        })
        .collect();
    assert!(seen.len() > 1); // got more than one distinct file across seeds
}

#[test]
fn test_pick_filler_path_reads_from_the_filler_folder() {
    let tmp = TmpDir::new();
    std::fs::create_dir(tmp.path().join("filler")).expect("create dir");
    std::fs::write(tmp.path().join("filler").join("hmm.wav"), b"fake").expect("write file");
    let result = pick_filler_path(tmp.path(), None);
    assert!(result.is_some());
    assert_eq!(must(result).file_name().expect("file name"), "hmm.wav");
}

#[test]
fn test_load_sound_map_keeps_a_list_of_kinds() {
    let tmp = TmpDir::new();
    let path = tmp.path().join("sounds.json");
    std::fs::write(
        &path,
        r#"{"emotion/laughing": ["laugh", "giggle"], "sleepy/yawn": "yawn"}"#,
    )
    .expect("write file");
    assert_eq!(
        load_sound_map(&path),
        SoundMap::from([
            (
                "emotion/laughing".to_owned(),
                SoundKind::from(["laugh", "giggle"])
            ),
            ("sleepy/yawn".to_owned(), SoundKind::from("yawn")),
        ])
    );
}

#[test]
fn test_pick_filler_path_never_picks_a_presence_line() {
    // WP-19 audit: "rude!" / "back!" / "zip it" were mixed into the filler
    // pool. They live in presence/ now, and even a stray copy left in
    // filler/ is never used as a slow-request filler.
    let tmp = TmpDir::new();
    std::fs::create_dir(tmp.path().join("filler")).expect("create dir");
    std::fs::create_dir(tmp.path().join("presence")).expect("create dir");
    std::fs::write(tmp.path().join("filler").join("hmm_1.wav"), b"fake").expect("write file");
    for name in ["back_1.wav", "zip_it_1.wav", "rude_1.wav"] {
        std::fs::write(tmp.path().join("filler").join(name), b"fake").expect("write file");
        std::fs::write(tmp.path().join("presence").join(name), b"fake").expect("write file");
    }
    let picks: HashSet<String> = (0..40)
        .map(|seed| {
            must(pick_filler_path(tmp.path(), Some(&SeededRng::new(seed))))
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(picks, HashSet::from(["hmm_1.wav".to_owned()]));
}

#[test]
fn test_pick_filler_path_none_when_only_presence_lines() {
    let tmp = TmpDir::new();
    std::fs::create_dir(tmp.path().join("presence")).expect("create dir");
    std::fs::write(tmp.path().join("presence").join("back_1.wav"), b"fake").expect("write file");
    assert!(pick_filler_path(tmp.path(), None).is_none());
}

#[test]
fn test_pick_presence_path_matches_the_line_prefix() {
    let tmp = TmpDir::new();
    std::fs::create_dir(tmp.path().join("presence")).expect("create dir");
    for name in ["back_1.wav", "back_2.wav", "zip_it_1.wav", "rude_1.wav"] {
        std::fs::write(tmp.path().join("presence").join(name), b"fake").expect("write file");
    }
    let backs: HashSet<String> = (0..20)
        .map(|s| {
            must(pick_presence_path(
                "back",
                tmp.path(),
                Some(&SeededRng::new(s)),
            ))
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned()
        })
        .collect();
    assert_eq!(
        backs,
        HashSet::from(["back_1.wav".to_owned(), "back_2.wav".to_owned()])
    );
    assert_eq!(
        must(pick_presence_path("zip_it", tmp.path(), None))
            .file_name()
            .expect("file name"),
        "zip_it_1.wav"
    );
    assert!(pick_presence_path("nope", tmp.path(), None).is_none());
}

#[test]
fn test_pick_kind_sound_picks_between_kinds_at_random() {
    let tmp = TmpDir::new();
    for kind in ["laugh", "giggle"] {
        let kind_dir = tmp.path().join(kind);
        std::fs::create_dir(&kind_dir).expect("create dir");
        std::fs::write(kind_dir.join(format!("{kind}_1.wav")), b"fake").expect("write file");
    }
    let kinds: HashSet<String> = (0..30)
        .map(|s| {
            must(pick_kind_sound(
                &SoundKind::from(["laugh", "giggle"]),
                tmp.path(),
                Some(&SeededRng::new(s)),
            ))
            .parent()
            .expect("kind dir")
            .file_name()
            .expect("kind name")
            .to_string_lossy()
            .into_owned()
        })
        .collect();
    assert_eq!(
        kinds,
        HashSet::from(["laugh".to_owned(), "giggle".to_owned()])
    );
}

#[test]
fn test_pick_kind_sound_falls_back_to_the_kind_that_has_files() {
    let tmp = TmpDir::new();
    std::fs::create_dir(tmp.path().join("giggle")).expect("create dir");
    std::fs::write(tmp.path().join("giggle").join("giggle_1.wav"), b"fake").expect("write file");
    for seed in 0..10 {
        assert_eq!(
            must(pick_kind_sound(
                &SoundKind::from(["laugh", "giggle"]),
                tmp.path(),
                Some(&SeededRng::new(seed)),
            ))
            .parent()
            .expect("kind dir")
            .file_name()
            .expect("kind name"),
            "giggle"
        );
    }
    assert!(pick_kind_sound(&SoundKind::from("laugh"), tmp.path(), None).is_none());
}
