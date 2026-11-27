//! Full-repository Git merge conflict scanner and structured resolver inspired by
//! Oh My Pi's `conflict://` workflow — powered by `omp-xfs` parallel walking,
//! `omp-diff` in-memory unified diffs, and atomic file writes (zero new deps).

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result, bail, ensure};
use omp_xfs::{
    FileType, Fs,
    glob::Selector,
    walk::{self, Action},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    diff_util::{unified_diff, write_atomic},
    path_guard::{display_relative, read_utf8_file, resolve_file_in_root, resolve_in_root, resolve_root},
};

const OURS_PREFIX: &str = "<<<<<<<";
const BASE_PREFIX: &str = "|||||||";
const SEPARATOR: &str = "=======";
const THEIRS_PREFIX: &str = ">>>>>>>";

const MAX_SCAN_FILE_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_MAX_CONFLICTS: usize = 200;
const MAX_DIFF_PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ConflictResolution {
    /// 1-based conflict index within `file` (as returned by `action: "scan"`).
    pub index: usize,
    /// Resolution strategy: `ours`, `theirs`, `base` (diff3 only), `both`, `both_reverse`, `delete`, or `custom`.
    pub choice: String,
    /// Custom replacement text when `choice` is `custom`. Supports standalone line tokens
    /// `@ours`, `@theirs`, `@base`, and `@both` to splice recorded sides cleanly.
    pub custom_text: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConflictsInput {
    /// Operation: `scan` (find conflicts in a file or across the repository) or `resolve` (resolve conflict(s) in `file`).
    pub action: String,
    /// Absolute project root directory.
    pub root: PathBuf,
    /// Target file or subdirectory inside `root`. Optional for `scan` (defaults to full repo); required file for `resolve`.
    pub file: Option<PathBuf>,
    /// Optional include glob when scanning a directory (e.g. `src/**/*.rs`).
    pub include: Option<String>,
    /// Optional exclude globs when scanning a directory.
    pub exclude: Option<Vec<String>>,
    /// Include dotfiles/hidden directories during directory scan (default: false).
    pub hidden: Option<bool>,
    /// Maximum total conflict blocks returned by `scan` (default: 200).
    pub max_conflicts: Option<usize>,
    /// Shorthand for `resolve`: 1-based conflict index to resolve.
    pub index: Option<usize>,
    /// Shorthand for `resolve`: apply `choice` to every conflict block in `file`.
    pub all: Option<bool>,
    /// Shorthand for `resolve`: `ours` | `theirs` | `base` | `both` | `both_reverse` | `delete` | `custom`.
    pub choice: Option<String>,
    /// Shorthand for `resolve`: replacement text when `choice == "custom"` (supports `@ours`/`@theirs`/`@base`/`@both`).
    pub custom_text: Option<String>,
    /// Batch resolutions for multiple conflict indices within `file` in a single atomic edit.
    pub resolutions: Option<Vec<ConflictResolution>>,
    /// When `false` (default), returns unified diff preview without touching disk.
    /// Set `true` to write the resolved file atomically.
    pub apply: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictBlock {
    pub index: usize,
    pub start_line: usize,
    pub base_line: Option<usize>,
    pub separator_line: usize,
    pub end_line: usize,
    pub ours_label: Option<String>,
    pub base_label: Option<String>,
    pub theirs_label: Option<String>,
    pub ours_lines: Vec<String>,
    pub base_lines: Option<Vec<String>>,
    pub theirs_lines: Vec<String>,
    pub has_crlf: bool,
}

/// Dispatch `git_conflicts` (`scan` or `resolve`).
pub async fn execute(input: ConflictsInput) -> Result<Value> {
    match input.action.trim().to_ascii_lowercase().as_str() {
        "scan" => scan_conflicts(input).await,
        "resolve" => resolve_conflicts(input),
        other => bail!("unsupported git_conflicts action '{other}'; use `scan` or `resolve`"),
    }
}

/// Strict column-0 Git conflict marker parser (supports 2-way and diff3 3-way blocks, LF and CRLF).
pub fn scan_conflict_lines(text: &str) -> Vec<ConflictBlock> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Idle,
        Ours,
        Base,
        Theirs,
    }

    struct Partial {
        start_line: usize,
        has_crlf: bool,
        ours_label: Option<String>,
        ours_lines: Vec<String>,
        base_line: Option<usize>,
        base_label: Option<String>,
        base_lines: Option<Vec<String>>,
        separator_line: Option<usize>,
        theirs_lines: Vec<String>,
    }

    let mut blocks = Vec::new();
    let mut phase = Phase::Idle;
    let mut partial: Option<Partial> = None;

    for (idx, raw_line) in text.split('\n').enumerate() {
        let line_no = idx + 1;
        let (line, has_cr) = match raw_line.strip_suffix('\r') {
            Some(stripped) => (stripped, true),
            None => (raw_line, false),
        };

        if let Some(label) = match_marker(line, OURS_PREFIX) {
            partial = Some(Partial {
                start_line: line_no,
                has_crlf: has_cr,
                ours_label: (!label.is_empty()).then(|| label.to_owned()),
                ours_lines: Vec::new(),
                base_line: None,
                base_label: None,
                base_lines: None,
                separator_line: None,
                theirs_lines: Vec::new(),
            });
            phase = Phase::Ours;
            continue;
        }

        if phase == Phase::Idle {
            continue;
        }

        if let Some(label) = match_marker(line, BASE_PREFIX) {
            if phase == Phase::Ours
                && let Some(ref mut p) = partial
            {
                p.base_line = Some(line_no);
                p.base_label = (!label.is_empty()).then(|| label.to_owned());
                p.base_lines = Some(Vec::new());
                phase = Phase::Base;
            } else {
                partial = None;
                phase = Phase::Idle;
            }
            continue;
        }

        if line == SEPARATOR {
            if matches!(phase, Phase::Ours | Phase::Base)
                && let Some(ref mut p) = partial
            {
                p.separator_line = Some(line_no);
                phase = Phase::Theirs;
            } else {
                partial = None;
                phase = Phase::Idle;
            }
            continue;
        }

        if let Some(label) = match_marker(line, THEIRS_PREFIX) {
            if phase == Phase::Theirs
                && let Some(p) = partial.take()
                && let Some(separator_line) = p.separator_line
            {
                blocks.push(ConflictBlock {
                    index: blocks.len() + 1,
                    start_line: p.start_line,
                    base_line: p.base_line,
                    separator_line,
                    end_line: line_no,
                    ours_label: p.ours_label,
                    base_label: p.base_label,
                    theirs_label: (!label.is_empty()).then(|| label.to_owned()),
                    ours_lines: p.ours_lines,
                    base_lines: p.base_lines,
                    theirs_lines: p.theirs_lines,
                    has_crlf: p.has_crlf,
                });
            } else {
                partial = None;
            }
            phase = Phase::Idle;
            continue;
        }

        if let Some(ref mut p) = partial {
            match phase {
                Phase::Ours => p.ours_lines.push(line.to_owned()),
                Phase::Base => {
                    if let Some(ref mut b) = p.base_lines {
                        b.push(line.to_owned());
                    }
                }
                Phase::Theirs => p.theirs_lines.push(line.to_owned()),
                Phase::Idle => {}
            }
        }
    }

    blocks
}

fn match_marker<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(prefix)?;
    if rest.is_empty() {
        return Some("");
    }
    rest.strip_prefix(' ')
}

async fn scan_conflicts(input: ConflictsInput) -> Result<Value> {
    let root = resolve_root(&input.root)?;
    let scope = match &input.file {
        Some(p) => resolve_in_root(&root, p)?,
        None => root.clone(),
    };
    let max_conflicts = input.max_conflicts.unwrap_or(DEFAULT_MAX_CONFLICTS).max(1);

    if scope.is_file() {
        let display = display_relative(&root, &scope);
        let text = read_utf8_file(&scope, MAX_SCAN_FILE_BYTES)?;
        let blocks = scan_conflict_lines(&text);
        let truncated = blocks.len() > max_conflicts;
        let shown: Vec<Value> = blocks
            .iter()
            .take(max_conflicts)
            .map(format_block)
            .collect();
        let files = if shown.is_empty() {
            Vec::new()
        } else {
            vec![json!({
                "path": display,
                "conflict_count": blocks.len(),
                "conflicts": shown,
            })]
        };
        return Ok(json!({
            "scope": display,
            "files_scanned": 1,
            "files_with_conflicts": usize::from(!blocks.is_empty()),
            "total_conflicts": blocks.len(),
            "truncated": truncated,
            "files": files,
        }));
    }

    let include = match input.include.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(pat) => Some(
            Selector::scope(pat, true)
                .map_err(|e| anyhow::anyhow!("invalid include glob '{pat}': {e}"))?,
        ),
        None => None,
    };
    let exclude = match input.exclude.as_deref() {
        Some(list) if !list.is_empty() => {
            Selector::exclude(list).map_err(|e| anyhow::anyhow!("invalid exclude glob: {e}"))?
        }
        _ => None,
    };

    let walk_opts = walk::Options {
        hidden: input.hidden.unwrap_or(false),
        gitignore: true,
        skip_git: true,
        skip_node_modules: true,
        follow_links: false,
        ..Default::default()
    };
    let xfs = Fs::native();
    let collected = walk::collect(&xfs, &scope, &walk_opts, true, |_, entry| {
        if entry.file_type == FileType::File {
            Action::Continue
        } else {
            Action::Skip
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("walk {}: {e}", scope.display()))?;

    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    for entry in collected.iter() {
        if let Some(ref inc) = include
            && !inc.matches(entry.path)
        {
            continue;
        }
        if let Some(ref exc) = exclude
            && exc.matches(entry.path)
        {
            continue;
        }
        let abs = scope.join(entry.rel_path());
        let display = display_relative(&root, &abs);
        candidates.push((abs, display));
    }

    let scan_result = tokio::task::spawn_blocking(move || {
        let mut files_scanned = 0usize;
        let mut files_with_conflicts = 0usize;
        let mut total_conflicts = 0usize;
        let mut shown_conflicts = 0usize;
        let mut truncated = false;
        let mut file_reports = Vec::new();

        for (abs, display) in candidates {
            let Ok(meta) = fs::metadata(&abs) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_SCAN_FILE_BYTES {
                continue;
            }
            files_scanned += 1;
            let Ok(bytes) = fs::read(&abs) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            if !text.contains(OURS_PREFIX) {
                continue;
            }
            let blocks = scan_conflict_lines(&text);
            if blocks.is_empty() {
                continue;
            }
            files_with_conflicts += 1;
            total_conflicts += blocks.len();

            if shown_conflicts < max_conflicts {
                let remaining = max_conflicts - shown_conflicts;
                let take_n = blocks.len().min(remaining);
                if take_n < blocks.len() {
                    truncated = true;
                }
                let formatted: Vec<Value> = blocks.iter().take(take_n).map(format_block).collect();
                shown_conflicts += take_n;
                file_reports.push(json!({
                    "path": display,
                    "conflict_count": blocks.len(),
                    "conflicts": formatted,
                }));
            } else {
                truncated = true;
            }
        }

        (
            files_scanned,
            files_with_conflicts,
            total_conflicts,
            truncated,
            file_reports,
        )
    })
    .await?;

    Ok(json!({
        "scope": display_relative(&root, &scope),
        "files_scanned": scan_result.0,
        "files_with_conflicts": scan_result.1,
        "total_conflicts": scan_result.2,
        "truncated": scan_result.3,
        "files": scan_result.4,
    }))
}

fn format_block(b: &ConflictBlock) -> Value {
    let mut obj = json!({
        "index": b.index,
        "start_line": b.start_line,
        "separator_line": b.separator_line,
        "end_line": b.end_line,
        "diff3": b.base_lines.is_some(),
        "ours_label": b.ours_label,
        "theirs_label": b.theirs_label,
        "ours": b.ours_lines.join("\n"),
        "theirs": b.theirs_lines.join("\n"),
    });
    if let Some(ref base) = b.base_lines {
        obj["base_line"] = json!(b.base_line);
        obj["base_label"] = json!(b.base_label);
        obj["base"] = json!(base.join("\n"));
    }
    obj
}

fn resolve_conflicts(input: ConflictsInput) -> Result<Value> {
    let root = resolve_root(&input.root)?;
    let file_arg = input
        .file
        .as_ref()
        .context("`file` is required for `git_conflicts` action `resolve`")?;
    let abs = resolve_file_in_root(&root, file_arg)?;
    let display = display_relative(&root, &abs);
    let before = read_utf8_file(&abs, MAX_SCAN_FILE_BYTES)?;
    let blocks = scan_conflict_lines(&before);
    ensure!(
        !blocks.is_empty(),
        "no unresolved Git merge conflicts found in {display}"
    );

    let plan = build_resolution_plan(&blocks, &input)?;
    let after = apply_resolutions_to_text(&before, &blocks, &plan)?;
    let remaining_blocks = scan_conflict_lines(&after);
    let apply = input.apply.unwrap_or(false);

    if apply && after != before {
        write_atomic(&abs, after.as_bytes())?;
    }

    let mut diff = unified_diff(&display, &before, &after);
    let mut diff_truncated = false;
    if diff.len() > MAX_DIFF_PREVIEW_BYTES {
        let cut = diff.floor_char_boundary(MAX_DIFF_PREVIEW_BYTES);
        diff.truncate(cut);
        diff.push_str("\n... [diff truncated]\n");
        diff_truncated = true;
    }

    Ok(json!({
        "applied": apply,
        "path": display,
        "resolved_indices": plan.keys().copied().collect::<Vec<_>>(),
        "resolved_count": plan.len(),
        "remaining_conflicts": remaining_blocks.len(),
        "diff": diff,
        "diff_truncated": diff_truncated,
    }))
}

fn build_resolution_plan(
    blocks: &[ConflictBlock],
    input: &ConflictsInput,
) -> Result<BTreeMap<usize, ConflictResolution>> {
    let mut plan = BTreeMap::new();

    if let Some(ref list) = input.resolutions {
        ensure!(
            !list.is_empty(),
            "`resolutions` must contain at least one entry"
        );
        let mut seen = BTreeSet::new();
        for item in list {
            ensure!(
                item.index >= 1 && item.index <= blocks.len(),
                "conflict index {} is out of range (1..={})",
                item.index,
                blocks.len()
            );
            ensure!(
                seen.insert(item.index),
                "duplicate conflict index {} in `resolutions`",
                item.index
            );
            plan.insert(item.index, item.clone());
        }
        return Ok(plan);
    }

    let choice = input
        .choice
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("provide either `resolutions` or `choice` (with `index` or `all: true`)")?;

    if input.all.unwrap_or(false) {
        for block in blocks {
            plan.insert(
                block.index,
                ConflictResolution {
                    index: block.index,
                    choice: choice.to_owned(),
                    custom_text: input.custom_text.clone(),
                },
            );
        }
        return Ok(plan);
    }

    let idx = input.index.unwrap_or_else(|| if blocks.len() == 1 { 1 } else { 0 });
    ensure!(
        idx >= 1 && idx <= blocks.len(),
        "provide `index` (1..={}) or `all: true` when file has multiple conflicts",
        blocks.len()
    );
    plan.insert(
        idx,
        ConflictResolution {
            index: idx,
            choice: choice.to_owned(),
            custom_text: input.custom_text.clone(),
        },
    );
    Ok(plan)
}

fn apply_resolutions_to_text(
    original: &str,
    blocks: &[ConflictBlock],
    plan: &BTreeMap<usize, ConflictResolution>,
) -> Result<String> {
    let mut raw_lines: Vec<String> = original.split('\n').map(str::to_owned).collect();

    // Splice from bottom to top so earlier line coordinates stay valid.
    for (&index, resolution) in plan.iter().rev() {
        let block = &blocks[index - 1];
        let mut replacement = render_replacement_lines(block, resolution)?;
        let start_idx = block.start_line - 1;
        let end_idx = block.end_line - 1;
        let has_following = end_idx + 1 < raw_lines.len();

        if block.has_crlf {
            let last = replacement.len().saturating_sub(1);
            for (i, line) in replacement.iter_mut().enumerate() {
                if i < last || has_following {
                    line.push('\r');
                }
            }
        }

        if replacement.is_empty() && !has_following && start_idx > 0 {
            if let Some(prev) = raw_lines[start_idx - 1].strip_suffix('\r') {
                raw_lines[start_idx - 1] = prev.to_owned();
            }
        }

        raw_lines.splice(start_idx..=end_idx, replacement);
    }

    Ok(raw_lines.join("\n"))
}

fn render_replacement_lines(
    block: &ConflictBlock,
    resolution: &ConflictResolution,
) -> Result<Vec<String>> {
    match resolution.choice.trim().to_ascii_lowercase().as_str() {
        "ours" => Ok(block.ours_lines.clone()),
        "theirs" => Ok(block.theirs_lines.clone()),
        "base" => block.base_lines.clone().with_context(|| {
            format!(
                "conflict #{} has no base section (2-way conflict); `base` requires diff3 markers (`|||||||`).",
                block.index
            )
        }),
        "both" => {
            let mut out = Vec::with_capacity(block.ours_lines.len() + block.theirs_lines.len());
            out.extend(block.ours_lines.iter().cloned());
            out.extend(block.theirs_lines.iter().cloned());
            Ok(out)
        }
        "both_reverse" => {
            let mut out = Vec::with_capacity(block.ours_lines.len() + block.theirs_lines.len());
            out.extend(block.theirs_lines.iter().cloned());
            out.extend(block.ours_lines.iter().cloned());
            Ok(out)
        }
        "delete" => Ok(Vec::new()),
        "custom" => {
            let raw = resolution
                .custom_text
                .as_deref()
                .context("`custom_text` is required when `choice` is `custom`")?;
            expand_custom_tokens(raw, block)
        }
        other => bail!(
            "unsupported conflict choice '{other}'; expected ours, theirs, base, both, both_reverse, delete, or custom"
        ),
    }
}

/// Expand OMP-style line tokens (`@ours`, `@theirs`, `@base`, `@both`) inside `custom_text`.
fn expand_custom_tokens(content: &str, block: &ConflictBlock) -> Result<Vec<String>> {
    if content.is_empty() {
        return Ok(Vec::new());
    }
    let normalized = content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .unwrap_or(content);

    let mut out = Vec::new();
    for raw_line in normalized.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        match line {
            "@ours" => out.extend(block.ours_lines.iter().cloned()),
            "@theirs" => out.extend(block.theirs_lines.iter().cloned()),
            "@base" => {
                let base = block.base_lines.as_ref().with_context(|| {
                    format!(
                        "conflict #{} has no base section (2-way conflict); `@base` requires diff3 markers.",
                        block.index
                    )
                })?;
                out.extend(base.iter().cloned());
            }
            "@both" => {
                out.extend(block.ours_lines.iter().cloned());
                out.extend(block.theirs_lines.iter().cloned());
            }
            _ => out.push(line.to_owned()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_way_and_diff3_and_ignores_false_markers() {
        let sample = "\
header\n\
<<<<<<<< not a marker\n\
<<<<<<< HEAD\n\
let a = 1;\n\
=======\n\
let a = 2;\n\
>>>>>>> feature\n\
middle\n\
<<<<<<< HEAD\n\
fn x() { 1 }\n\
||||||| base\n\
fn x() { 0 }\n\
=======\n\
fn x() { 2 }\n\
>>>>>>> other\n";

        let blocks = scan_conflict_lines(sample);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start_line, 3);
        assert_eq!(blocks[0].end_line, 7);
        assert_eq!(blocks[0].ours_label.as_deref(), Some("HEAD"));
        assert_eq!(blocks[0].theirs_label.as_deref(), Some("feature"));
        assert_eq!(blocks[0].ours_lines, vec!["let a = 1;"]);
        assert_eq!(blocks[0].theirs_lines, vec!["let a = 2;"]);
        assert!(blocks[0].base_lines.is_none());

        assert_eq!(blocks[1].start_line, 9);
        assert_eq!(blocks[1].base_line, Some(11));
        assert_eq!(blocks[1].base_label.as_deref(), Some("base"));
        assert_eq!(blocks[1].base_lines.as_deref(), Some(&["fn x() { 0 }".to_string()][..]));
    }

    #[tokio::test]
    async fn repo_scan_and_structured_resolve_preserve_crlf_and_dry_run() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        fs::create_dir_all(root.join("src"))?;
        let crlf_doc = "top\r\n<<<<<<< HEAD\r\nours_1\r\n||||||| base\r\nbase_1\r\n=======\r\ntheirs_1\r\n>>>>>>> topic\r\nmid\r\n<<<<<<< HEAD\r\nours_2\r\n=======\r\ntheirs_2\r\n>>>>>>> topic\r\nend\r\n";
        fs::write(root.join("src/merge.rs"), crlf_doc)?;
        fs::write(root.join("src/clean.rs"), "fn ok() {}\n")?;

        let scanned = execute(ConflictsInput {
            action: "scan".into(),
            root: root.clone(),
            file: None,
            include: None,
            exclude: None,
            hidden: None,
            max_conflicts: None,
            index: None,
            all: None,
            choice: None,
            custom_text: None,
            resolutions: None,
            apply: None,
        })
        .await?;
        assert_eq!(scanned["files_with_conflicts"], 1);
        assert_eq!(scanned["total_conflicts"], 2);
        assert_eq!(scanned["files"][0]["path"], "src/merge.rs");
        assert_eq!(scanned["files"][0]["conflicts"][0]["diff3"], true);

        // Dry-run resolve using batch resolutions including custom token expansion
        let preview = execute(ConflictsInput {
            action: "resolve".into(),
            root: root.clone(),
            file: Some(PathBuf::from("src/merge.rs")),
            include: None,
            exclude: None,
            hidden: None,
            max_conflicts: None,
            index: None,
            all: None,
            choice: None,
            custom_text: None,
            resolutions: Some(vec![
                ConflictResolution {
                    index: 1,
                    choice: "custom".into(),
                    custom_text: Some("@ours\n// merged\n@theirs".into()),
                },
                ConflictResolution {
                    index: 2,
                    choice: "theirs".into(),
                    custom_text: None,
                },
            ]),
            apply: Some(false),
        })
        .await?;
        assert_eq!(preview["applied"], false);
        assert_eq!(preview["resolved_count"], 2);
        assert_eq!(preview["remaining_conflicts"], 0);
        assert!(preview["diff"].as_str().unwrap().contains("+// merged"));
        // Disk unchanged on dry-run
        assert_eq!(fs::read_to_string(root.join("src/merge.rs"))?, crlf_doc);

        // Apply writes atomically and preserves CRLF
        let applied = execute(ConflictsInput {
            action: "resolve".into(),
            root: root.clone(),
            file: Some(PathBuf::from("src/merge.rs")),
            include: None,
            exclude: None,
            hidden: None,
            max_conflicts: None,
            index: None,
            all: None,
            choice: None,
            custom_text: None,
            resolutions: Some(vec![
                ConflictResolution {
                    index: 1,
                    choice: "base".into(),
                    custom_text: None,
                },
                ConflictResolution {
                    index: 2,
                    choice: "both".into(),
                    custom_text: None,
                },
            ]),
            apply: Some(true),
        })
        .await?;
        assert_eq!(applied["applied"], true);
        assert_eq!(applied["remaining_conflicts"], 0);
        let written = fs::read_to_string(root.join("src/merge.rs"))?;
        assert_eq!(
            written,
            "top\r\nbase_1\r\nmid\r\nours_2\r\ntheirs_2\r\nend\r\n"
        );
        Ok(())
    }
}
