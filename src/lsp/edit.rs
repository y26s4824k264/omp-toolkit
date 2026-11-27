//! Cross-platform LSP `WorkspaceEdit` validation, in-memory diff preview, and atomic apply.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use crate::{
    diff_util::{unified_diff, unified_rename_diff, write_atomic},
    path_guard::{
        display_relative, file_uri_to_path, read_utf8_file, resolve_file_in_root, utf16_col_to_utf8,
    },
};

const MAX_EDIT_FILES: usize = 256;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DIFF_PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct TextEdit {
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
    pub new_text: String,
}

#[derive(Debug, Clone)]
pub struct FileMove {
    pub from_abs: PathBuf,
    pub to_abs: PathBuf,
    pub from_display: String,
    pub to_display: String,
}

/// Extract per-URI text edits from one or more LSP `WorkspaceEdit` values.
pub fn collect_workspace_edits(edits: &[Value]) -> Result<BTreeMap<String, Vec<TextEdit>>> {
    let mut by_uri: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
    for ws in edits {
        if ws.is_null() {
            continue;
        }
        if let Some(changes) = ws.get("changes").and_then(Value::as_object) {
            for (uri, list) in changes {
                let arr = list
                    .as_array()
                    .with_context(|| format!("invalid WorkspaceEdit.changes for {uri}"))?;
                let target = by_uri.entry(uri.clone()).or_default();
                for item in arr {
                    target.push(parse_text_edit(item)?);
                }
            }
        }
        if let Some(doc_changes) = ws.get("documentChanges").and_then(Value::as_array) {
            for op in doc_changes {
                if op.get("kind").is_some() {
                    bail!("WorkspaceEdit resource operations (create/delete/rename) are handled explicitly");
                }
                let uri = op
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .context("WorkspaceEdit documentChange missing textDocument.uri")?;
                let arr = op
                    .get("edits")
                    .and_then(Value::as_array)
                    .context("WorkspaceEdit documentChange missing edits")?;
                let target = by_uri.entry(uri.to_owned()).or_default();
                for item in arr {
                    target.push(parse_text_edit(item)?);
                }
            }
        }
    }
    ensure!(
        by_uri.len() <= MAX_EDIT_FILES,
        "WorkspaceEdit touches {} files (limit {MAX_EDIT_FILES})",
        by_uri.len()
    );
    Ok(by_uri)
}

/// Validate, render unified diff preview, and optionally atomically apply `WorkspaceEdit` + optional file move.
pub fn apply_or_preview_workspace_edits(
    root: &Path,
    by_uri: BTreeMap<String, Vec<TextEdit>>,
    file_move: Option<FileMove>,
    apply: bool,
) -> Result<Value> {
    let mut staged: Vec<(PathBuf, String, String, String, usize)> = Vec::new();
    let mut moved_before_after: Option<(String, String)> = None;

    for (uri, edits) in by_uri {
        if edits.is_empty() {
            continue;
        }
        let raw_path = file_uri_to_path(&uri)?;
        let abs = resolve_file_in_root(root, &raw_path)?;
        let display = display_relative(root, &abs);
        let before = read_utf8_file(&abs, MAX_FILE_BYTES)?;
        let after = apply_text_edits(&before, &edits)
            .with_context(|| format!("apply LSP text edits to {display}"))?;
        if let Some(ref mv) = file_move
            && abs == mv.from_abs
        {
            moved_before_after = Some((before, after));
            continue;
        }
        if before != after {
            staged.push((abs, display, before, after, edits.len()));
        }
    }

    if let Some(ref mv) = file_move
        && moved_before_after.is_none()
    {
        let before = read_utf8_file(&mv.from_abs, MAX_FILE_BYTES)?;
        moved_before_after = Some((before.clone(), before));
    }

    if apply {
        for (abs, _, _, after, _) in &staged {
            write_atomic(abs, after.as_bytes())?;
        }
        if let Some(ref mv) = file_move {
            ensure!(
                !mv.to_abs.exists(),
                "rename destination already exists: {}",
                mv.to_display
            );
            if let Some((_, ref after)) = moved_before_after {
                write_atomic(&mv.from_abs, after.as_bytes())?;
            }
            fs::rename(&mv.from_abs, &mv.to_abs).with_context(|| {
                format!("rename {} -> {}", mv.from_display, mv.to_display)
            })?;
        }
    }

    let mut diff = String::new();
    let mut diff_truncated = false;
    let mut files = Vec::new();
    let mut total_edits = 0usize;

    for (_, display, before, after, count) in &staged {
        total_edits += count;
        files.push(json!({ "path": display, "edits": count }));
        append_diff(&mut diff, &mut diff_truncated, &unified_diff(display, before, after));
    }

    if let (Some(mv), Some((before, after))) = (&file_move, &moved_before_after) {
        files.push(json!({
            "path": mv.from_display,
            "renamed_to": mv.to_display,
            "content_changed": before != after,
        }));
        append_diff(
            &mut diff,
            &mut diff_truncated,
            &unified_rename_diff(&mv.from_display, &mv.to_display, before, after),
        );
    }

    Ok(json!({
        "applied": apply,
        "files_changed": files.len(),
        "total_edits": total_edits,
        "files": files,
        "diff": diff,
        "diff_truncated": diff_truncated,
    }))
}

fn append_diff(target: &mut String, truncated: &mut bool, chunk: &str) {
    if *truncated || chunk.is_empty() {
        return;
    }
    if target.len() + chunk.len() <= MAX_DIFF_PREVIEW_BYTES {
        target.push_str(chunk);
    } else {
        let remaining = MAX_DIFF_PREVIEW_BYTES.saturating_sub(target.len());
        let cut = chunk.floor_char_boundary(remaining);
        target.push_str(&chunk[..cut]);
        target.push_str("\n... [diff truncated]\n");
        *truncated = true;
    }
}

fn parse_text_edit(val: &Value) -> Result<TextEdit> {
    let start_line = val
        .pointer("/range/start/line")
        .and_then(Value::as_u64)
        .context("TextEdit missing range.start.line")? as u32;
    let start_char = val
        .pointer("/range/start/character")
        .and_then(Value::as_u64)
        .context("TextEdit missing range.start.character")? as u32;
    let end_line = val
        .pointer("/range/end/line")
        .and_then(Value::as_u64)
        .context("TextEdit missing range.end.line")? as u32;
    let end_char = val
        .pointer("/range/end/character")
        .and_then(Value::as_u64)
        .context("TextEdit missing range.end.character")? as u32;
    let new_text = val
        .get("newText")
        .and_then(Value::as_str)
        .context("TextEdit missing newText")?
        .to_owned();
    ensure!(!new_text.contains('\0'), "TextEdit contains NUL byte");
    Ok(TextEdit {
        start_line,
        start_char,
        end_line,
        end_char,
        new_text,
    })
}

pub fn apply_text_edits(source: &str, edits: &[TextEdit]) -> Result<String> {
    let mut line_starts = vec![0usize];
    for (idx, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(idx + 1);
        }
    }
    let line_slice = |line: u32| -> Result<&str> {
        let idx = line as usize;
        ensure!(idx < line_starts.len(), "LSP line {line} out of bounds");
        let start = line_starts[idx];
        let end = if idx + 1 < line_starts.len() {
            line_starts[idx + 1] - 1
        } else {
            source.len()
        };
        Ok(&source[start..end])
    };

    let mut byte_edits: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for edit in edits {
        let s_line = line_slice(edit.start_line)?;
        let e_line = line_slice(edit.end_line)?;
        let start = line_starts[edit.start_line as usize] + utf16_col_to_utf8(s_line, edit.start_char)?;
        let end = line_starts[edit.end_line as usize] + utf16_col_to_utf8(e_line, edit.end_char)?;
        ensure!(start <= end, "LSP TextEdit has inverted range");
        byte_edits.push((start, end, edit.new_text.as_str()));
    }
    byte_edits.sort_by_key(|(start, end, _)| (*start, *end));

    let mut out = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for (start, end, replacement) in byte_edits {
        ensure!(start >= cursor, "LSP server returned overlapping text edits");
        out.push_str(&source[cursor..start]);
        out.push_str(replacement);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_multiple_utf16_text_edits_accurately() -> Result<()> {
        let src = "const emoji = \"🙂\"; oldName();\noldName();\n";
        let col = crate::path_guard::utf8_col_to_utf16(src.lines().next().unwrap(), src.find("oldName").unwrap());
        let edits = vec![
            TextEdit {
                start_line: 0,
                start_char: col,
                end_line: 0,
                end_char: col + 7,
                new_text: "newName".into(),
            },
            TextEdit {
                start_line: 1,
                start_char: 0,
                end_line: 1,
                end_char: 7,
                new_text: "newName".into(),
            },
        ];
        let out = apply_text_edits(src, &edits)?;
        assert_eq!(out, "const emoji = \"🙂\"; newName();\nnewName();\n");
        Ok(())
    }
}
