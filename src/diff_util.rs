//! In-memory Git unified diff rendering (powered by `omp-diff`) and atomic file writes.

use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Component, Path},
};

use anyhow::{Context, Result};
use omp_diff::{HunkLine, diff_line_tokens, line_tokens_str, structured_patch_hunk_rows};
use tempfile::NamedTempFile;

/// Render a standard Git unified diff between `before` and `after` for a single file.
pub fn unified_diff(display_path: &str, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let old_tokens = line_tokens_str(before);
    let new_tokens = line_tokens_str(after);
    let runs = diff_line_tokens(&old_tokens, &new_tokens);
    let hunks = structured_patch_hunk_rows(Some(3), &old_tokens, &new_tokens, &runs);

    let old_p = quoted_path('a', Path::new(display_path));
    let new_p = quoted_path('b', Path::new(display_path));
    let mut out = String::new();
    let _ = writeln!(out, "diff --git {old_p} {new_p}");
    let _ = writeln!(out, "--- {old_p}");
    let _ = writeln!(out, "+++ {new_p}");
    for hunk in hunks {
        let old_start = if hunk.old_lines == 0 {
            hunk.old_start.saturating_sub(1)
        } else {
            hunk.old_start
        };
        let new_start = if hunk.new_lines == 0 {
            hunk.new_start.saturating_sub(1)
        } else {
            hunk.new_start
        };
        let _ = writeln!(
            out,
            "@@ -{old_start},{} +{new_start},{} @@",
            hunk.old_lines, hunk.new_lines
        );
        for row in hunk.lines {
            match row {
                HunkLine::Context(text) => {
                    let _ = writeln!(out, " {text}");
                }
                HunkLine::Removed(text) => {
                    let _ = writeln!(out, "-{text}");
                }
                HunkLine::Added(text) => {
                    let _ = writeln!(out, "+{text}");
                }
                HunkLine::NoNewlineAtEof => {
                    let _ = writeln!(out, "\\ No newline at end of file");
                }
            }
        }
    }
    out
}

/// Render a rename diff header + content diff when moving `old_display` to `new_display`.
pub fn unified_rename_diff(
    old_display: &str,
    new_display: &str,
    before: &str,
    after: &str,
) -> String {
    let old_p = quoted_path('a', Path::new(old_display));
    let new_p = quoted_path('b', Path::new(new_display));
    let mut out = String::new();
    let _ = writeln!(out, "diff --git {old_p} {new_p}");
    let _ = writeln!(out, "rename from {old_display}");
    let _ = writeln!(out, "rename to {new_display}");
    if before != after {
        let old_tokens = line_tokens_str(before);
        let new_tokens = line_tokens_str(after);
        let runs = diff_line_tokens(&old_tokens, &new_tokens);
        let hunks = structured_patch_hunk_rows(Some(3), &old_tokens, &new_tokens, &runs);
        let _ = writeln!(out, "--- {old_p}");
        let _ = writeln!(out, "+++ {new_p}");
        for hunk in hunks {
            let old_start = if hunk.old_lines == 0 {
                hunk.old_start.saturating_sub(1)
            } else {
                hunk.old_start
            };
            let new_start = if hunk.new_lines == 0 {
                hunk.new_start.saturating_sub(1)
            } else {
                hunk.new_start
            };
            let _ = writeln!(
                out,
                "@@ -{old_start},{} +{new_start},{} @@",
                hunk.old_lines, hunk.new_lines
            );
            for row in hunk.lines {
                match row {
                    HunkLine::Context(text) => {
                        let _ = writeln!(out, " {text}");
                    }
                    HunkLine::Removed(text) => {
                        let _ = writeln!(out, "-{text}");
                    }
                    HunkLine::Added(text) => {
                        let _ = writeln!(out, "+{text}");
                    }
                    HunkLine::NoNewlineAtEof => {
                        let _ = writeln!(out, "\\ No newline at end of file");
                    }
                }
            }
        }
    }
    out
}

/// Preserve original CRLF line endings when the original file used uniform CRLF.
pub fn preserve_line_endings(before: &str, after: String) -> String {
    let before_crlf = before.contains("\r\n") && !has_bare_lf(before);
    if before_crlf && has_bare_lf(&after) {
        after.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        after
    }
}

fn has_bare_lf(s: &str) -> bool {
    s.split_inclusive('\n')
        .any(|line| line.ends_with('\n') && !line.ends_with("\r\n"))
}

/// Atomically write `content` to `path` via temp file + rename in the same directory,
/// preserving existing file permissions on Unix.
pub fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("resolve parent of {}", path.display()))?;
    let existing_perms = fs::metadata(path).ok().map(|m| m.permissions());
    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("create temp file in {}", parent.display()))?;
    temp.write_all(content)
        .with_context(|| format!("write temp content for {}", path.display()))?;
    temp.as_file().sync_all()?;
    if let Some(perms) = existing_perms {
        let _ = fs::set_permissions(temp.path(), perms);
    }
    temp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("atomically replace {}", path.display()))?;
    Ok(())
}

fn quoted_path(side: char, path: &Path) -> String {
    let mut parts = Vec::new();
    for comp in path.components() {
        if let Component::Normal(s) = comp {
            parts.push(s.to_string_lossy().into_owned());
        }
    }
    let joined = format!("{side}/{}", parts.join("/"));
    if joined
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\'))
    {
        return joined;
    }
    let mut quoted = String::with_capacity(joined.len() + 4);
    quoted.push('"');
    for &b in joined.as_bytes() {
        match b {
            b'"' | b'\\' => {
                quoted.push('\\');
                quoted.push(char::from(b));
            }
            b' '..=b'~' => quoted.push(char::from(b)),
            _ => {
                let _ = write!(quoted, "\\{b:03o}");
            }
        }
    }
    quoted.push('"');
    quoted
}
