//! Windows-style path helpers and fnmatch globbing - pure, OS-independent
//! (`ntpath` semantics explicitly, so behaviour matches the Python module
//! on any OS).

/// Windows-style normalized, case-folded path - the tracking key.
/// `ntpath.normcase(ntpath.normpath(path.strip()))`.
pub fn norm_path(path: &str) -> String {
    ntpath_normcase(&ntpath_normpath(path.trim()))
}

/// Lowercased final path component (`ntpath.basename(path).lower()`).
pub fn exe_name_of(path: &str) -> String {
    ntpath_basename(path).to_lowercase()
}

/// Is `path_key` the root itself or inside it? Both args are norm_path'd.
pub fn is_under(path_key: &str, root_key: &str) -> bool {
    let root = root_key.trim_end_matches('\\');
    path_key == root
        || path_key
            .strip_prefix(root)
            .is_some_and(|r| r.starts_with('\\'))
}

/// `fnmatch.fnmatchcase` - case-sensitive glob match on `*`, `?` and
/// `[seq]` / `[!seq]` character classes.
pub fn fnmatch_case(name: &str, pattern: &str) -> bool {
    fn matches(name: &[char], pat: &[char]) -> bool {
        let Some(&head) = pat.first() else {
            return name.is_empty();
        };
        match head {
            '*' => (0..=name.len()).any(|i| matches(&name[i..], &pat[1..])),
            '?' => !name.is_empty() && matches(&name[1..], &pat[1..]),
            '[' => {
                let Some(&c) = name.first() else {
                    return false;
                };
                let mut j = 1;
                let negated = pat.get(1) == Some(&'!');
                if negated {
                    j = 2;
                }
                if pat.get(j) == Some(&']') {
                    j += 1; // a leading ']' is literal
                }
                let class_start = j;
                while j < pat.len() && pat[j] != ']' {
                    j += 1;
                }
                if j >= pat.len() {
                    // Unclosed class: '[' is literal (fnmatch.translate).
                    return c == '[' && matches(&name[1..], &pat[1..]);
                }
                let class = &pat[class_start..j];
                let mut hit = false;
                let mut k = 0;
                while k < class.len() {
                    if k + 2 < class.len() && class[k + 1] == '-' {
                        if class[k] <= c && c <= class[k + 2] {
                            hit = true;
                        }
                        k += 3;
                    } else {
                        if class[k] == c {
                            hit = true;
                        }
                        k += 1;
                    }
                }
                hit != negated && matches(&name[1..], &pat[j + 1..])
            }
            _ => !name.is_empty() && name[0] == head && matches(&name[1..], &pat[1..]),
        }
    }
    matches(
        &name.chars().collect::<Vec<_>>(),
        &pattern.chars().collect::<Vec<_>>(),
    )
}

/// `ntpath.basename`: final component after the last `\` or `/`.
pub(crate) fn ntpath_basename(path: &str) -> &str {
    let start = path.rfind(['\\', '/']).map_or(0, |i| i + 1);
    &path[start..]
}

/// `ntpath.splitext`: split the final component at its last `.`, ignoring
/// a leading dot (hidden-file convention).
pub(crate) fn ntpath_splitext(path: &str) -> (&str, &str) {
    let base_start = path.rfind(['\\', '/']).map_or(0, |i| i + 1);
    let base = &path[base_start..];
    match base.rfind('.') {
        Some(d) if d > 0 => path.split_at(base_start + d),
        _ => (path, ""),
    }
}

/// `ntpath.join` for the fixed 3-piece case used by `build_sources`
/// (plus generic pieces): later absolute/drive components reset.
pub(crate) fn ntpath_join(base: &str, parts: &[&str]) -> String {
    let mut acc = base.replace('/', "\\");
    for part in parts {
        let part = part.replace('/', "\\");
        if part.starts_with('\\') || (part.len() >= 2 && part.as_bytes()[1] == b':') {
            acc = part;
        } else if acc.is_empty() || acc.ends_with('\\') || acc.ends_with(':') {
            acc.push_str(&part);
        } else {
            acc.push('\\');
            acc.push_str(&part);
        }
    }
    acc
}

fn ntpath_normcase(path: &str) -> String {
    path.to_lowercase()
}

fn ntpath_normpath(path: &str) -> String {
    if path.starts_with(r"\\.\") || path.starts_with(r"\\?\") {
        return path.to_string(); // Win32 special prefixes are not normalized
    }
    let path = path.replace('/', "\\");
    let (prefix, rest) = ntpath_splitdrive(&path);
    let rooted = rest.starts_with('\\');
    let mut comps: Vec<&str> = Vec::new();
    for comp in rest.split('\\') {
        match comp {
            "" | "." => {}
            ".." => match comps.last() {
                Some(&last) if last != ".." => {
                    comps.pop();
                }
                _ if !rooted => comps.push(".."),
                _ => {} // rooted paths cannot climb above the root
            },
            c => comps.push(c),
        }
    }
    let mut out = String::from(prefix);
    if rooted {
        out.push('\\');
    }
    out.push_str(&comps.join("\\"));
    if out.is_empty() {
        out.push('.');
    }
    out
}

fn ntpath_splitdrive(path: &str) -> (&str, &str) {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return path.split_at(2);
    }
    // UNC: \\server\share is the drive.
    if path.starts_with(r"\\") && !path.starts_with(r"\\\") {
        let after = &path[2..];
        if let Some(server_end) = after.find('\\') {
            let server = &after[..server_end];
            let rest = &after[server_end + 1..];
            if !server.is_empty() {
                if let Some(share_end) = rest.find('\\') {
                    if !rest[..share_end].is_empty() {
                        return path.split_at(2 + server_end + 1 + share_end);
                    }
                } else if !rest.is_empty() {
                    return (path, "");
                }
            }
        }
    }
    ("", path)
}
