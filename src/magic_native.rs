use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use omp_ast::summary::parse_code_status;
use omp_edit::{
    notebook::is_notebook_path,
    text::{detect_line_ending, normalize_to_lf, restore_line_endings, strip_bom},
};
use serde::{Deserialize, Serialize};

use crate::{Change, edit_path_policy, resolve_project_file};

const MAX_FILES: usize = 16;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_BATCH_BYTES: usize = 4 * MAX_FILE_BYTES;
// JSON may use six bytes for a single byte in an input or output snapshot.
const MAX_JSON_BYTES: usize = 12 * MAX_BATCH_BYTES + MAX_FILE_BYTES;

#[derive(Serialize)]
struct Snapshot {
    path: PathBuf,
    before: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    path: PathBuf,
    before: String,
    after: String,
}

pub(crate) fn snapshot(root: &Path, files: &[PathBuf]) -> Result<()> {
    let snapshots = load_snapshots(root, files)?;
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &snapshots)?;
    writeln!(stdout)?;
    Ok(())
}

pub(crate) fn preview(root: &Path, input: impl Read) -> Result<Vec<Change>> {
    let mut bytes = Vec::new();
    input
        .take(MAX_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_JSON_BYTES,
        "Magic edit input is too large"
    );
    let replacements: Vec<Replacement> =
        serde_json::from_slice(&bytes).context("parse Magic edit JSON")?;
    let paths: Vec<_> = replacements
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let snapshots = load_snapshots(root, &paths)?;
    let mut changes = Vec::new();
    let mut total = 0;

    for (snapshot, replacement) in snapshots.into_iter().zip(replacements) {
        let path = snapshot.path;
        let display = path.to_str().context("Magic requires a UTF-8 file name")?;
        ensure!(
            snapshot.before == replacement.before,
            "source changed since Magic prepare: {display}"
        );
        let after = replacement.after;
        ensure!(
            after.len() <= MAX_FILE_BYTES,
            "Magic output is too large: {display}"
        );
        total += after.len();
        ensure!(total <= MAX_BATCH_BYTES, "Magic output batch is too large");
        ensure!(
            !after.contains('\0') && !after.contains("\r\n") && !after.starts_with('\u{feff}'),
            "expected LF text without BOM for {display}"
        );

        let (bom, body) = strip_bom(&snapshot.before);
        let normalized = normalize_to_lf(body);
        if normalized == after {
            continue;
        }
        if parses(&normalized, display)? {
            ensure!(
                parses(&after, display)?,
                "Magic edit produces a syntax error in {display}"
            );
        }
        let mut persisted = String::from(bom);
        persisted.push_str(&restore_line_endings(&after, detect_line_ending(body)));
        changes.push(Change {
            path,
            before: snapshot.before,
            after: persisted,
        });
    }
    Ok(changes)
}

fn parses(source: &str, path: &str) -> Result<bool> {
    let source = if source.is_empty() { "\n" } else { source };
    Ok(parse_code_status(source, None, Some(path))?.is_some())
}

pub(crate) fn check_syntax(path: &Path, input: impl Read) -> Result<()> {
    let display = path.to_str().context("Magic requires a UTF-8 file name")?;
    let mut source = String::new();
    input
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_string(&mut source)
        .context("read Magic source for syntax diagnostics")?;
    ensure!(
        source.len() <= MAX_FILE_BYTES,
        "Magic source is too large: {display}"
    );
    println!("{}", parses(&source, display)?);
    Ok(())
}

fn load_snapshots(root: &Path, files: &[PathBuf]) -> Result<Vec<Snapshot>> {
    ensure!(
        (1..=MAX_FILES).contains(&files.len()),
        "Magic requires between 1 and {MAX_FILES} files"
    );
    let policy = edit_path_policy(root);
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut snapshots = Vec::with_capacity(files.len());
    for path in files {
        let display = path.to_str().context("Magic requires a UTF-8 file name")?;
        let absolute = resolve_project_file(root, path)?;
        let canonical = absolute
            .canonicalize()
            .with_context(|| format!("resolve {display}"))?;
        ensure!(
            canonical.starts_with(root),
            "source is outside the project: {display}"
        );
        ensure!(seen.insert(canonical), "duplicate Magic source: {display}");
        ensure!(
            !is_notebook_path(path),
            "Magic requires a text source: {display}"
        );

        let mut before = String::new();
        File::open(&absolute)?
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_string(&mut before)
            .with_context(|| format!("read UTF-8 source {display}"))?;
        ensure!(
            before.len() <= MAX_FILE_BYTES,
            "Magic input is too large: {display}"
        );
        ensure!(!before.contains('\0'), "expected text source: {display}");
        let has_crlf = before.contains("\r\n");
        ensure!(
            !has_crlf
                || !before
                    .split_inclusive('\n')
                    .any(|line| line.ends_with('\n') && !line.ends_with("\r\n")),
            "cannot preserve mixed LF and CRLF line endings in Magic preview for {display}"
        );
        let relative = absolute
            .strip_prefix(root)?
            .to_str()
            .context("Magic requires a UTF-8 file name")?;
        if let Some(message) =
            policy.auto_generated_message(relative, &before.as_bytes()[..before.len().min(1024)])
        {
            anyhow::bail!("{message}");
        }
        total += before.len();
        ensure!(total <= MAX_BATCH_BYTES, "Magic input batch is too large");
        snapshots.push(Snapshot {
            path: path.clone(),
            before,
        });
    }
    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn request(before: &str, after: &str) -> String {
        serde_json::json!([{ "path": "sample.ts", "before": before, "after": after }]).to_string()
    }

    #[test]
    fn preview_preserves_bom_and_crlf_and_rejects_stale_or_invalid_source() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path().canonicalize()?;
        let file = root.join("sample.ts");
        let before = "\u{feff}export const result = 1;\r\n";
        fs::write(&file, before)?;
        let input = request(before, "export const result = 2;\n");
        let changes = preview(&root, input.as_bytes())?;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].after, "\u{feff}export const result = 2;\r\n");
        assert_eq!(fs::read_to_string(file)?, before);
        let stale = request("export const result = 1;\n", "export const result = 2;\n");
        assert!(
            preview(&root, stale.as_bytes())
                .err()
                .unwrap()
                .to_string()
                .contains("source changed")
        );
        let invalid = request(before, "export const result = ;\n");
        assert!(
            preview(&root, invalid.as_bytes())
                .err()
                .unwrap()
                .to_string()
                .contains("syntax error")
        );
        assert!(load_snapshots(&root, &[PathBuf::from("../sample.ts")]).is_err());
        fs::write(root.join("mixed.txt"), "first\r\nsecond\n")?;
        assert!(
            load_snapshots(&root, &[PathBuf::from("mixed.txt")])
                .err()
                .unwrap()
                .to_string()
                .contains("mixed LF and CRLF")
        );
        fs::write(root.join("model.pb.go"), "package model\n")?;
        assert!(load_snapshots(&root, &[PathBuf::from("model.pb.go/")]).is_err());
        Ok(())
    }
}
