//! Unified path containment, URI conversion, and UTF-16 position helpers shared
//! across AST, LSP, DAP, and output-minimization modules.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use regex::RegexBuilder;

/// Resolve and validate an absolute project root directory.
pub fn resolve_root(root: &Path) -> Result<PathBuf> {
    let canonical = root
        .canonicalize()
        .with_context(|| format!("resolve project root {}", root.display()))?;
    ensure!(
        canonical.is_dir(),
        "project root is not a directory: {}",
        canonical.display()
    );
    Ok(canonical)
}

/// Resolve a file or directory inside `root` (relative to `root` or already inside `root`),
/// forbidding `.git` traversal and escapes outside `root`.
pub fn resolve_in_root(root: &Path, input: &Path) -> Result<PathBuf> {
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let canonical = joined
        .canonicalize()
        .with_context(|| format!("resolve path {}", input.display()))?;
    ensure!(
        canonical.starts_with(root),
        "path escapes project root: {}",
        input.display()
    );
    ensure!(
        !contains_git_dir(&canonical, root),
        "access to .git metadata is not allowed: {}",
        input.display()
    );
    Ok(canonical)
}

/// Resolve an existing regular file inside `root`.
pub fn resolve_file_in_root(root: &Path, input: &Path) -> Result<PathBuf> {
    let path = resolve_in_root(root, input)?;
    ensure!(
        path.is_file(),
        "expected an existing file inside root: {}",
        input.display()
    );
    Ok(path)
}

/// Resolve a destination path inside `root` whose leaf file may not exist yet.
pub fn resolve_new_path_in_root(root: &Path, input: &Path) -> Result<PathBuf> {
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let mut normalized = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(
                    normalized.pop(),
                    "destination escapes filesystem root: {}",
                    input.display()
                );
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let parent = normalized
        .parent()
        .with_context(|| format!("invalid destination path: {}", input.display()))?;
    let file_name = normalized
        .file_name()
        .with_context(|| format!("destination missing file name: {}", input.display()))?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("destination parent does not exist: {}", parent.display()))?;
    ensure!(
        canonical_parent.starts_with(root),
        "destination escapes project root: {}",
        input.display()
    );
    let final_path = canonical_parent.join(file_name);
    ensure!(
        !contains_git_dir(&final_path, root),
        "cannot write into .git directory: {}",
        input.display()
    );
    Ok(final_path)
}

/// Format a path relative to `root` with forward slashes for display & patches.
pub fn display_relative(root: &Path, absolute: &Path) -> String {
    let rel = absolute.strip_prefix(root).unwrap_or(absolute);
    let mut parts = Vec::new();
    for comp in rel.components() {
        if let Component::Normal(s) = comp {
            parts.push(s.to_string_lossy().into_owned());
        }
    }
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

/// Read a bounded UTF-8 text file safely.
pub fn read_utf8_file(path: &Path, max_bytes: u64) -> Result<String> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    ensure!(meta.is_file(), "not a regular file: {}", path.display());
    ensure!(
        meta.len() <= max_bytes,
        "file exceeds {max_bytes} byte limit: {}",
        path.display()
    );
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(
        !bytes.contains(&0),
        "expected UTF-8 text file (NUL byte found): {}",
        path.display()
    );
    String::from_utf8(bytes).with_context(|| format!("file is not valid UTF-8: {}", path.display()))
}

fn contains_git_dir(path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().any(|c| match c {
        Component::Normal(name) => name.eq_ignore_ascii_case(".git"),
        _ => false,
    })
}

/// Convert a canonical filesystem path into a standard `file://` URI (RFC 8089).
pub fn path_to_file_uri(path: &Path) -> Result<String> {
    let raw = path
        .to_str()
        .with_context(|| format!("non-UTF-8 path cannot form file URI: {}", path.display()))?;
    let normalized = raw.replace('\\', "/");
    let mut out = String::with_capacity(normalized.len() + 16);
    if normalized.starts_with('/') {
        out.push_str("file://");
    } else {
        out.push_str("file:///");
    }
    for (idx, seg) in normalized.split('/').enumerate() {
        if idx > 0 {
            out.push('/');
        }
        // Keep Windows drive colon (`C:`) unescaped in the first segment.
        if idx == 0
            && seg.len() == 2
            && seg.as_bytes()[0].is_ascii_alphabetic()
            && seg.as_bytes()[1] == b':'
        {
            out.push_str(seg);
            continue;
        }
        for &b in seg.as_bytes() {
            if is_uri_unreserved(b) {
                out.push(char::from(b));
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    Ok(out)
}

/// Convert a `file://` URI back into a local `PathBuf`.
pub fn file_uri_to_path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .with_context(|| format!("expected file:// URI, got {uri}"))?;
    let path_part = if let Some(stripped) = rest.strip_prefix("localhost/") {
        format!("/{stripped}")
    } else {
        rest.to_owned()
    };
    let decoded = percent_decode(&path_part)?;
    #[cfg(windows)]
    let decoded = {
        let b = decoded.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            decoded[1..].replace('/', "\\")
        } else {
            decoded.replace('/', "\\")
        }
    };
    Ok(PathBuf::from(decoded))
}

fn is_uri_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn percent_decode(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            ensure!(i + 2 < bytes.len(), "truncated percent escape in URI");
            let hi = hex_val(bytes[i + 1])?;
            let lo = hex_val(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).context("URI path is not valid UTF-8")
}

fn hex_val(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => bail!("invalid hex digit in URI escape"),
    }
}

/// Convert a UTF-8 byte column within a line into an LSP UTF-16 code-unit offset.
pub fn utf8_col_to_utf16(line: &str, byte_col: usize) -> u32 {
    let clamped = byte_col.min(line.len());
    let prefix = &line[..line.floor_char_boundary(clamped)];
    prefix.encode_utf16().count() as u32
}

/// Convert an LSP UTF-16 code-unit offset within a line into a UTF-8 byte index.
pub fn utf16_col_to_utf8(line: &str, utf16_col: u32) -> Result<usize> {
    let clean = line.strip_suffix('\r').unwrap_or(line);
    let mut units = 0u32;
    for (byte_idx, ch) in clean.char_indices() {
        if units == utf16_col {
            return Ok(byte_idx);
        }
        let w = ch.len_utf16() as u32;
        ensure!(
            units + w <= utf16_col,
            "UTF-16 column {utf16_col} splits a surrogate pair"
        );
        units += w;
    }
    ensure!(
        units == utf16_col,
        "UTF-16 column {utf16_col} exceeds line length ({units})"
    );
    Ok(clean.len())
}

/// Resolve a 0-based LSP `(line, utf16_character)` from either explicit 1-based `line`/`column`
/// or a `symbol` specifier (`name` or `name#occurrence`, optionally constrained to 1-based `line`).
pub fn resolve_lsp_position(
    text: &str,
    line: Option<u32>,
    column: Option<u32>,
    symbol: Option<&str>,
) -> Result<(u32, u32)> {
    let lines: Vec<&str> = text.split('\n').collect();
    if let Some(line_num) = line {
        ensure!(
            line_num >= 1 && (line_num as usize) <= lines.len(),
            "line {line_num} is outside document (1..={})",
            lines.len()
        );
    }

    if let Some(sym_spec) = symbol.map(str::trim).filter(|s| !s.is_empty()) {
        let (sym, occurrence) = parse_symbol_spec(sym_spec);
        let escaped = regex::escape(sym);
        let is_ident = !sym.is_empty()
            && sym
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
        let pattern = if is_ident {
            format!(r"\b{escaped}\b")
        } else {
            escaped
        };

        let start = line.map_or(0, |l| (l - 1) as usize);
        let end = line.map_or(lines.len(), |l| l as usize);

        for case_insensitive in [false, true] {
            let re = RegexBuilder::new(&pattern)
                .case_insensitive(case_insensitive)
                .build()
                .context("compile symbol lookup regex")?;
            let mut seen = 0usize;
            for (idx, row) in lines.iter().enumerate().take(end).skip(start) {
                for m in re.find_iter(row) {
                    seen += 1;
                    if seen == occurrence {
                        return Ok((idx as u32, utf8_col_to_utf16(row, m.start())));
                    }
                }
            }
            if seen > 0 {
                break;
            }
        }
        bail!("symbol '{sym_spec}' not found in document");
    }

    let line_num = line.context("provide either `symbol` or `line`")?;
    let row = lines[(line_num - 1) as usize];
    if let Some(col_num) = column {
        ensure!(col_num >= 1, "column is 1-based");
        let byte_idx = row
            .char_indices()
            .nth((col_num - 1) as usize)
            .map_or(row.len(), |(idx, _)| idx);
        return Ok((line_num - 1, utf8_col_to_utf16(row, byte_idx)));
    }
    let first_non_ws = row.find(|c: char| !c.is_whitespace()).unwrap_or(0);
    Ok((line_num - 1, utf8_col_to_utf16(row, first_non_ws)))
}

fn parse_symbol_spec(spec: &str) -> (&str, usize) {
    if let Some((name, num)) = spec.rsplit_once('#')
        && !name.is_empty()
        && let Ok(n) = num.parse::<usize>()
        && n >= 1
    {
        return (name, n);
    }
    (spec, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip_and_utf16_conversion() -> Result<()> {
        #[cfg(windows)]
        let (p, expected_prefix) = (
            Path::new(r"C:\tmp\hello world\测试.rs"),
            "file:///C:/tmp/hello%20world/",
        );
        #[cfg(not(windows))]
        let (p, expected_prefix) = (
            Path::new("/tmp/hello world/测试.rs"),
            "file:///tmp/hello%20world/",
        );
        let uri = path_to_file_uri(p)?;
        assert!(uri.starts_with(expected_prefix), "unexpected uri: {uri}");
        assert_eq!(file_uri_to_path(&uri)?, p);

        let line = "let s = \"🙂\"; foo();";
        let foo_byte = line.find("foo").unwrap();
        let u16_col = utf8_col_to_utf16(line, foo_byte);
        // "🙂" is 4 UTF-8 bytes and 2 UTF-16 code units.
        assert_eq!(utf16_col_to_utf8(line, u16_col)?, foo_byte);
        Ok(())
    }

    #[test]
    fn symbol_position_supports_occurrences_and_line_filter() -> Result<()> {
        let text = "fn alpha() {}\nlet x = alpha() + alpha();\n";
        assert_eq!(resolve_lsp_position(text, None, None, Some("alpha"))?, (0, 3));
        assert_eq!(
            resolve_lsp_position(text, Some(2), None, Some("alpha#2"))?,
            (1, 18)
        );
        Ok(())
    }
}
