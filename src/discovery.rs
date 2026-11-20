use std::{
    collections::{BTreeMap, HashSet},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use anyhow::{Context, Result, ensure};
use omp_ast::{
    ops::resolve_supported_lang,
    search::{Capture, Match, MatchStrictness, Page, Query, Span, find_source},
    summary::{SummaryOptions, SummarySegment, summarize_code},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::read_project_file;

const MAX_SEARCH_LIMIT: usize = 50;
const MATCH_TEXT_BYTES: usize = 256;
const SUMMARY_LINES: usize = 100;
const SUMMARY_LINE_BYTES: usize = 320;
const SUMMARY_TEXT_BYTES: usize = 6_500;

pub(crate) fn ast_find(
    root: &Path,
    language: &str,
    pattern: &str,
    files: &[PathBuf],
    offset: usize,
    limit: usize,
) -> Result<()> {
    ensure!(!files.is_empty(), "list the project files to search");
    ensure!(!pattern.trim().is_empty(), "AST pattern was empty");
    ensure!(
        (1..=MAX_SEARCH_LIMIT).contains(&limit),
        "limit must be 1 to {MAX_SEARCH_LIMIT}"
    );
    let language = resolve_supported_lang(language)?;
    let patterns = [pattern.to_owned()];
    let query = Query {
        patterns: &patterns,
        lang: None,
        selector: None,
        strictness: MatchStrictness::Smart,
        captures: true,
    };
    let cancel = CancellationToken::new();
    let mut seen = HashSet::new();
    #[cfg(unix)]
    let mut identities = HashSet::new();
    let mut matches = Vec::new();
    let mut warnings = Vec::new();
    let mut total_matches = 0;

    for file in files {
        let (absolute, source) = read_project_file(root, file)?;
        let path = absolute.strip_prefix(root)?;
        ensure!(
            seen.insert(path.to_path_buf()),
            "duplicate file: {}",
            path.display()
        );
        #[cfg(unix)]
        {
            let metadata = absolute.metadata()?;
            ensure!(
                identities.insert((metadata.dev(), metadata.ino())),
                "more than one path refers to the same file: {}",
                path.display()
            );
        }
        let page = Page {
            offset: offset.saturating_sub(total_matches),
            limit: limit - matches.len(),
        };
        let found = find_source(&source, language, &query, page, &cancel)
            .with_context(|| format!("search {}", path.display()))?;
        total_matches += found.total_matches;
        matches.extend(found.matches.iter().map(|entry| format_match(path, entry)));
        warnings.extend(
            found
                .warnings
                .iter()
                .map(|warning| json!({"path": path, "message": clipped(warning, 400)})),
        );
    }
    let next_offset = offset.saturating_add(matches.len());
    write_json(json!({
        "language": language.canonical_name(),
        "files_searched": seen.len(),
        "complete": warnings.is_empty(),
        "offset": offset,
        "limit": limit,
        "total_matches": total_matches,
        "next_offset": (next_offset < total_matches).then_some(next_offset),
        "end_exclusive": true,
        "matches": matches,
        "warnings": warnings,
    }))
}

fn format_match(path: &Path, matched: &Match) -> Value {
    let captures = matched.captures.as_ref().map(|captures| {
        let single: BTreeMap<_, _> = captures
            .single
            .iter()
            .map(|(name, capture)| (name, format_capture(capture)))
            .collect();
        let multi: BTreeMap<_, Vec<_>> = captures
            .multi
            .iter()
            .map(|(name, captures)| (name, captures.iter().map(format_capture).collect()))
            .collect();
        json!({"single": single, "multi": multi})
    });
    json!({
        "path": path,
        "range": format_span(matched.span),
        "text": clipped(&matched.text, MATCH_TEXT_BYTES),
        "captures": captures,
    })
}

fn format_capture(capture: &Capture) -> Value {
    json!({
        "range": format_span(capture.span),
        "text": clipped(&capture.text, MATCH_TEXT_BYTES),
    })
}

fn format_span(span: Span) -> Value {
    json!({
        "start_line": span.start_pos.line + 1,
        "start_column": span.start_pos.column + 1,
        "end_line": span.end_pos.line + 1,
        "end_column": span.end_pos.column + 1,
        "start_byte": span.start,
        "end_byte": span.end,
    })
}

pub(crate) fn read_summary(root: &Path, file: &Path) -> Result<()> {
    let (absolute, code) = read_project_file(root, file)?;
    let path = absolute.strip_prefix(root)?;
    let summary = summarize_code(SummaryOptions {
        code,
        lang: None,
        path: Some(absolute.to_string_lossy().into_owned()),
        min_body_lines: None,
        min_comment_lines: None,
        unfold_until_lines: None,
        unfold_limit_lines: None,
    })
    .with_context(|| format!("summarize {}", path.display()))?;
    let (segments, lines_clipped) = bound_summary(summary.segments);
    let last_line = segments.last().map_or(0, |segment| segment.end_line);
    let fallback = (!summary.parsed).then_some(if summary.total_lines == 0 {
        "empty source"
    } else {
        "grammar unavailable or syntax errors; showing source lines"
    });
    write_json(json!({
        "path": path,
        "language": summary.language,
        "parsed": summary.parsed,
        "fallback": fallback,
        "total_lines": summary.total_lines,
        "elided": summary.elided,
        "lines_clipped": lines_clipped,
        "omitted_after_preview": (last_line < summary.total_lines).then(|| json!({
            "start_line": last_line + 1,
            "end_line": summary.total_lines,
        })),
        "segments": segments,
    }))
}

fn bound_summary(segments: Vec<SummarySegment>) -> (Vec<SummarySegment>, bool) {
    let mut remaining_lines = SUMMARY_LINES;
    let mut remaining_bytes = SUMMARY_TEXT_BYTES;
    let mut lines_clipped = false;
    let mut visible = Vec::new();
    for mut segment in segments {
        if let Some(text) = segment.text.take() {
            let source_lines = (segment.end_line - segment.start_line + 1) as usize;
            let mut lines = Vec::new();
            for source in text.split('\n').take(source_lines.min(remaining_lines)) {
                let source = source.strip_suffix('\r').unwrap_or(source);
                let line = clipped(source, SUMMARY_LINE_BYTES);
                let bytes = line.len() + 1;
                if bytes > remaining_bytes {
                    break;
                }
                lines_clipped |= source.len() > SUMMARY_LINE_BYTES;
                remaining_bytes -= bytes;
                lines.push(line);
            }
            if lines.is_empty() {
                break;
            }
            let partial = lines.len() < source_lines;
            remaining_lines -= lines.len();
            segment.end_line = segment.start_line + lines.len() as u32 - 1;
            segment.text = Some(lines.join("\n"));
            visible.push(segment);
            if partial {
                break;
            }
        } else {
            visible.push(segment);
        }
    }
    (visible, lines_clipped)
}

fn clipped(source: &str, limit: usize) -> String {
    if source.len() <= limit {
        return source.to_owned();
    }
    let end = source.floor_char_boundary(limit - '…'.len_utf8());
    format!("{}…", &source[..end])
}

fn write_json(value: Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &value)?;
    writeln!(stdout)?;
    Ok(())
}
