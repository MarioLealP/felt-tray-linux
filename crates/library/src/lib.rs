//! Scans felt-overlay's real animation library on disk for the actual set of
//! valid tags, instead of hardcoding a tag list anywhere in felt-animation -
//! the library keeps growing as Mario adds Mixamo content, and felt-overlay's
//! own tag convention is already "folder nesting under animations/library/ IS
//! the tag, no manifest file" (main.gd's _scan_library_dir), so this just reads
//! the same filesystem structure felt-overlay itself reads.
//!
//! A directory counts as a real tag only if it directly contains at least one
//! `.vrma` or `.vrma.import` file (mirrors main.gd's dual-suffix matching for
//! exported PCK builds) - an empty intermediate folder like "reaction/" on its
//! own (with only subfolders, no clips) is not itself a playable tag.

use std::collections::HashSet;
use std::path::Path;

/// True when a directory entry is a clip file: a `.vrma`, or the
/// `.vrma.import` sidecar left next to it in exported PCK builds.
fn is_clip_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = file_name_str(path);
    if name.ends_with(".vrma.import") {
        return true;
    }
    path.extension().and_then(|ext| ext.to_str()) == Some("vrma")
}

/// File name of `path` as a `str` when valid UTF-8 (the Python original
/// works on `str` names throughout).
fn file_name_str(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// The clip stem Python computes as
/// `name.removesuffix(".import").removesuffix(".vrma")` -
/// "drink_water.vrma" and "drink_water.vrma.import" both yield "drink_water".
fn clip_stem(name: &str) -> &str {
    let name = name.strip_suffix(".import").unwrap_or(name);
    name.strip_suffix(".vrma").unwrap_or(name)
}

/// Whether `dir` directly contains at least one clip file (subfolders don't
/// count - "reaction/" with only "reaction/weather/" under it is not a tag).
fn dir_has_clip(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| is_clip_file(&entry.path()))
}

/// `Path::as_posix()` equivalent: components joined with `/`.
fn to_posix(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Recursively walk `dir`, adding every directory (relative to `root`, as a
/// posix-style path) that directly contains at least one clip file.
fn collect_tags(root: &Path, dir: &Path, tags: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if dir_has_clip(&path) {
            // Result iterates as 0..=1 items, so a path outside the root is
            // simply skipped rather than recorded under a broken key.
            tags.extend(path.strip_prefix(root).map(to_posix));
        }
        collect_tags(root, &path, tags);
    }
}

/// Walk `animations_lib` and return every directory that directly holds at
/// least one `.vrma` / `.vrma.import` clip, keyed by its path relative to the
/// root in posix form ("reaction/weather/sun"). A missing root yields the
/// empty set; the root itself is never a tag even if clips sit directly in it.
pub fn scan_tags(animations_lib: &Path) -> HashSet<String> {
    if !animations_lib.is_dir() {
        return HashSet::new();
    }
    let mut tags = HashSet::new();
    collect_tags(animations_lib, animations_lib, &mut tags);
    tags
}

/// felt-core can't know the desktop's animation tags (it just guesses a
/// plausible path, e.g. get_weather emitting "reaction/weather/sun" when no
/// "reaction/weather" folder exists on this machine) - so any tag arriving
/// from the server has to be checked against what's actually on disk before
/// it's trusted. Exact match wins; otherwise walk up the path
/// ("a/b/c" -> "a/b" -> "a") to the nearest ancestor that's a real tag,
/// since a broader real tag is still a better reaction than nothing. None
/// if nothing along that chain matches, or if no tag was supplied at all -
/// the caller falls through to classify_reaction() in that case.
pub fn resolve_known_tag<'a>(
    tag: Option<&'a str>,
    known_tags: &HashSet<String>,
) -> Option<&'a str> {
    let tag = tag.filter(|t| !t.is_empty())?;
    if known_tags.contains(tag) {
        return Some(tag);
    }
    let mut candidate = tag;
    while let Some(pos) = candidate.rfind('/') {
        candidate = &candidate[..pos];
        if known_tags.contains(candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Resolves a semantic tag against the current posture (standing/sitting).
///
/// The library's existing convention (set by hand tonight, e.g. idle/,
/// locomotion/walk/) is that a bare tag IS the standing/default form - only
/// a sitting-specific variant needs an explicit "<tag>/sitting" subfolder.
/// So: standing always just wants the bare tag. Sitting prefers
/// "<tag>/sitting" if it actually exists, and otherwise falls back to the
/// bare (standing) tag - which requires standing up first (Mario,
/// 2026-09-21: "stand up then play, latency is fine").
///
/// WP-20: an upper-body tag (`upper_body`, main.gd's "upper" class) plays
/// on the upper layer over the sitting legs, so it never needs a stand-up -
/// a sitting variant is still preferred when one exists.
///
/// Returns `(resolved_tag, needs_stand_up_first)`.
pub fn resolve_posture_tag(
    tag: &str,
    posture: &str,
    known_tags: &HashSet<String>,
    upper_body: bool,
) -> (String, bool) {
    if posture == "sitting" {
        let sitting_variant = format!("{tag}/sitting");
        if known_tags.contains(&sitting_variant) {
            return (sitting_variant, false);
        }
        return (tag.to_string(), !upper_body);
    }
    (tag.to_string(), false)
}

/// WP-21: the clip ids directly inside a tag folder (file stems, the
/// same ids main.gd registers - "drink_water" for ambient/drink_water.vrma),
/// so a prop can ask for the exact clip it was fitted to. Sorted, and
/// deduplicated across the `.vrma` / `.vrma.import` dual-suffix pair.
pub fn clips_in(animations_lib: &Path, tag: &str) -> Vec<String> {
    let folder = animations_lib.join(tag);
    if !folder.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut names = HashSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if is_clip_file(&path) {
            names.insert(clip_stem(file_name_str(&path)).to_string());
        }
    }
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    names
}
