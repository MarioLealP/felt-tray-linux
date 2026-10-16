//! Window-title cleanup: a human game name from a window title.

use std::sync::LazyLock;

use regex::Regex;

use crate::paths::{ntpath_basename, ntpath_splitext};

static STORE_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*[-–—|]\s*(steam|epic games|gog galaxy)\s*$").unwrap());
static BRACKET_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*[\(\[]([^\(\)\[\]]*)[\)\]]\s*$").unwrap());
static BRACKET_JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(v?\d[\d.,\s]*|.*\b(\d+-?bit|x64|x86|dx\s?\d+|d3d\s?\d+|directx\s?\d*|vulkan|opengl|build|version|shipping|debug|release|development|beta|alpha)\b.*)\s*$",
    )
    .unwrap()
});
static VERSION_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*[-–—|:]?\s*\b(v(ersion)?\.?\s*\d+(\.\d+)*[a-z]?|\d+(\.\d+){1,3}[a-z]?|build\s*\d+(\.\d+)*)\s*$",
    )
    .unwrap()
});
static TRAILING_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s\-–—|:]+$").unwrap());
static STEM_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([-_](win64|wingdk|win32))?[-_]shipping$|[-_](x64|x86|dx11|dx12|64)$").unwrap()
});

/// Pure: a human game name from a window title, e.g.
/// 'MONSTER HUNTER: WORLD(421810)' -> 'MONSTER HUNTER: WORLD',
/// 'Factorio 2.0.55' -> 'Factorio'. Falls back to the exe stem
/// ('AC2-Win64-Shipping.exe' -> 'AC2') when there's no usable title.
pub fn clean_title(title: Option<&str>, exe_path_or_name: Option<&str>) -> String {
    let mut text = title
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for _ in 0..4 {
        // a few passes: suffixes can stack
        let before = text.clone();
        text = STORE_SUFFIX.replace_all(&text, "").into_owned();
        if let Some(m) = BRACKET_SUFFIX.captures(&text) {
            let whole = m.get(0).unwrap();
            let inner = m.get(1).unwrap().as_str();
            if BRACKET_JUNK.is_match(inner) {
                text = text[..whole.start()].to_string();
            }
        }
        text = VERSION_SUFFIX.replace_all(&text, "").into_owned();
        text = TRAILING_SEP.replace_all(&text, "").into_owned();
        if text == before {
            break;
        }
    }
    if text.chars().count() >= 2 && text.chars().any(|c| c.is_alphabetic()) {
        return text;
    }
    let fallback = exe_path_or_name.unwrap_or("");
    let (stem, _) = ntpath_splitext(ntpath_basename(fallback));
    let cleaned = STEM_SUFFIX.replace_all(stem, "").into_owned();
    if cleaned.is_empty() {
        stem.to_string()
    } else {
        cleaned
    }
}
