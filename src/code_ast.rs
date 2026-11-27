//! High-density code reading (`summary` & `outline`) and full-repository
//! structural AST search & rewrite powered natively by `omp-ast` + `omp-xfs`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use omp_ast::{
    SupportLang,
    block::{BlockRangeOptions, NodeSpan, block_range_at, node_chain_at},
    language::grammar::{self, LanguageGrammar},
    ops::{resolve_language, resolve_supported_lang, supported_lang_list},
    outline::{self, Item, SymbolType},
    search::{
        Capture, Discover, Match, MatchStrictness, Mode, Page, Query, Rewrite, discover,
        find_page, rewrite,
    },
    summary::{SummaryOptions, SummarySegment, summarize_code},
    vue::parse_sfc_blocks,
};
use omp_vfs::Fs;
use omp_xfs::glob::Selector;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    diff_util::{preserve_line_endings, unified_diff, write_atomic},
    path_guard::{display_relative, read_utf8_file, resolve_file_in_root, resolve_in_root, resolve_root},
};

const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MATCH_SNIPPET_BYTES: usize = 320;
const MAX_DIFF_PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SummaryInput {
    /// Absolute project root directory.
    pub root: PathBuf,
    /// Project-relative (or in-root) source file path.
    pub file: PathBuf,
    /// Optional explicit language alias (inferred from extension if omitted).
    pub lang: Option<String>,
    /// Optional 1-based line number to expand into its full tree-sitter syntactic block
    /// (includes Rust outer attributes/doc comments and Python/TS/Java decorators).
    /// When set, returns that complete block with exact line numbers instead of a folded file summary.
    pub block_line: Option<u32>,
    /// Target visible line budget for progressive BFS unfolding (0 = outermost fold only).
    pub unfold_until_lines: Option<u32>,
    /// Hard ceiling for BFS unfolding visible lines.
    pub unfold_limit_lines: Option<u32>,
    /// Minimum body lines before folding (default 4).
    pub min_body_lines: Option<u32>,
    /// Minimum block comment lines before folding (default 6).
    pub min_comment_lines: Option<u32>,
    /// Render as compact numbered text instead of JSON (default: true for concise token usage).
    pub compact: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OutlineInput {
    /// Absolute project root directory.
    pub root: PathBuf,
    /// Project-relative (or in-root) source file path.
    pub file: PathBuf,
    /// Optional explicit language alias (inferred from extension if omitted).
    pub lang: Option<String>,
    /// Optional symbol type filters (e.g. ["function", "struct", "class", "method"]).
    pub types: Option<Vec<String>>,
    /// Include import declarations (default: false).
    pub include_imports: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AstGrepInput {
    /// Absolute project root directory.
    pub root: PathBuf,
    /// Optional file or subdirectory inside `root` to scope the search (defaults to `root`).
    pub path: Option<PathBuf>,
    /// One or more ast-grep patterns (e.g. `foo($ARG)`, `async fn $NAME($$$ARGS)`).
    pub patterns: Vec<String>,
    /// Optional explicit language alias. Omit to auto-detect per file extension across the repo.
    pub lang: Option<String>,
    /// Optional AST node kind selector for fragment patterns (e.g. `pair`, `field_declaration`).
    pub selector: Option<String>,
    /// Optional include glob (e.g. `src/**/*.rs`, `*.ts`).
    pub include: Option<String>,
    /// Optional exclude globs (e.g. `["*.test.ts", "fixtures"]`).
    pub exclude: Option<Vec<String>>,
    /// Match strictness: `smart` (default), `cst`, `ast`, `relaxed`, `signature`.
    pub strictness: Option<String>,
    /// Include dotfiles/hidden directories (default: false).
    pub hidden: Option<bool>,
    /// Matches to skip for pagination (default: 0).
    pub offset: Option<usize>,
    /// Page size, 1 to 100 (default: 30).
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RewriteRule {
    /// AST match pattern.
    pub pattern: String,
    /// Replacement template referencing metavariables from `pattern`.
    pub rewrite: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AstEditInput {
    /// Absolute project root directory.
    pub root: PathBuf,
    /// Optional file or subdirectory inside `root` to scope the rewrite (defaults to `root`).
    pub path: Option<PathBuf>,
    /// Ordered list of `{ pattern, rewrite }` structural rules.
    pub rules: Vec<RewriteRule>,
    /// Optional explicit language alias. Omit to auto-detect per file extension.
    pub lang: Option<String>,
    /// Optional AST node kind selector for fragment patterns.
    pub selector: Option<String>,
    /// Optional include glob (e.g. `src/**/*.ts`).
    pub include: Option<String>,
    /// Optional exclude globs.
    pub exclude: Option<Vec<String>>,
    /// Match strictness: `smart` (default), `cst`, `ast`, `relaxed`, `signature`.
    pub strictness: Option<String>,
    /// Drop overlapping matches instead of failing (default: false).
    pub skip_overlaps: Option<bool>,
    /// Maximum replacements across all files (default: 500).
    pub max_replacements: Option<usize>,
    /// Maximum modified files (default: 100).
    pub max_files: Option<usize>,
    /// When `false` (default), returns unified diff preview without touching disk.
    /// Set `true` to validate syntax and atomically write all modified files.
    pub apply: Option<bool>,
}

/// Fold a source file into a compact AST-aware structural summary, or expand a
/// single syntactic block around `block_line` using `omp_ast::block`.
pub fn code_summary(input: SummaryInput) -> Result<String> {
    let root = resolve_root(&input.root)?;
    let abs = resolve_file_in_root(&root, &input.file)?;
    let display = display_relative(&root, &abs);
    let code = read_utf8_file(&abs, MAX_READ_BYTES)?;

    if let Some(target_line) = input.block_line {
        return render_syntactic_block(
            &display,
            &abs.to_string_lossy(),
            &code,
            input.lang,
            target_line,
            input.compact.unwrap_or(true),
        );
    }

    let summary = summarize_code(SummaryOptions {
        code,
        lang: input.lang,
        path: Some(abs.to_string_lossy().into_owned()),
        min_body_lines: input.min_body_lines,
        min_comment_lines: input.min_comment_lines,
        unfold_until_lines: input.unfold_until_lines.or(Some(120)),
        unfold_limit_lines: input.unfold_limit_lines.or(Some(240)),
    })
    .with_context(|| format!("summarize {display}"))?;

    if input.compact.unwrap_or(true) {
        Ok(render_compact_summary(&display, &summary.language, summary.parsed, summary.total_lines, &summary.segments))
    } else {
        Ok(serde_json::to_string_pretty(&json!({
            "path": display,
            "language": summary.language,
            "parsed": summary.parsed,
            "elided": summary.elided,
            "total_lines": summary.total_lines,
            "segments": summary.segments,
        }))?)
    }
}

fn render_syntactic_block(
    display: &str,
    abs_path: &str,
    code: &str,
    lang: Option<String>,
    target_line: u32,
    compact: bool,
) -> Result<String> {
    let lines: Vec<&str> = code.split('\n').collect();
    let total_lines = lines.len() as u32;
    ensure!(
        target_line >= 1 && target_line <= total_lines,
        "block_line {target_line} is outside {display} (1..={total_lines})"
    );

    let exact = block_range_at(BlockRangeOptions {
        code: code.to_owned(),
        lang: lang.clone(),
        path: Some(abs_path.to_owned()),
        line: target_line,
    })
    .with_context(|| format!("resolve syntactic block at {display}:{target_line}"))?;

    let chain: Vec<NodeSpan> = node_chain_at(BlockRangeOptions {
        code: code.to_owned(),
        lang,
        path: Some(abs_path.to_owned()),
        line: target_line,
    })
    .with_context(|| format!("resolve enclosing AST chain at {display}:{target_line}"))?
    .unwrap_or_default();

    let (start_line, end_line, resolution) = if let Some(range) = exact {
        (range.start_line, range.end_line, "exact")
    } else if let Some(enclosing) = chain
        .iter()
        .find(|span| span.end_line > span.start_line)
        .or_else(|| chain.first())
    {
        (enclosing.start_line, enclosing.end_line, "enclosing")
    } else {
        bail!(
            "no syntactic block found at {display}:{target_line} (line may be blank, unsupported language, or inside a syntax error)"
        );
    };

    let start_idx = (start_line.saturating_sub(1) as usize).min(lines.len());
    let end_idx = (end_line as usize).min(lines.len());
    let slice = &lines[start_idx..end_idx];

    if compact {
        use std::fmt::Write;
        let width = end_line.max(1).to_string().len();
        let mut out = format!(
            "# {display}:L{start_line}-L{end_line} (block at line {target_line}, {resolution})\n"
        );
        for (offset, raw) in slice.iter().enumerate() {
            let line_no = start_line + offset as u32;
            let clean = raw.strip_suffix('\r').unwrap_or(raw);
            let _ = writeln!(out, "{line_no:>width$}| {clean}");
        }
        Ok(out)
    } else {
        let text = slice
            .iter()
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(serde_json::to_string_pretty(&json!({
            "path": display,
            "block_line": target_line,
            "start_line": start_line,
            "end_line": end_line,
            "resolution": resolution,
            "total_lines": total_lines,
            "enclosing_chain": chain,
            "text": text,
        }))?)
    }
}

fn render_compact_summary(
    display: &str,
    language: &Option<String>,
    parsed: bool,
    total_lines: u32,
    segments: &[SummarySegment],
) -> String {
    use std::fmt::Write;
    let lang = language.as_deref().unwrap_or("unknown");
    let mut out = format!(
        "# {display} ({lang}, {total_lines} lines{})\n",
        if parsed { "" } else { ", syntax fallback" }
    );
    let width = total_lines.max(1).to_string().len();
    for seg in segments {
        if let Some(text) = &seg.text {
            for (offset, line) in text.split('\n').enumerate() {
                let line_no = seg.start_line + offset as u32;
                let clean = line.strip_suffix('\r').unwrap_or(line);
                let _ = writeln!(out, "{line_no:>width$}| {clean}");
            }
        } else {
            let count = seg.end_line.saturating_sub(seg.start_line) + 1;
            let _ = writeln!(
                out,
                "{:>width$}| ... ({count} lines elided: L{}-L{})",
                "", seg.start_line, seg.end_line
            );
        }
    }
    out
}

/// Extract top-level symbols and members from a source file using tree-sitter.
pub fn code_outline(input: OutlineInput) -> Result<Value> {
    let root = resolve_root(&input.root)?;
    let abs = resolve_file_in_root(&root, &input.file)?;
    let display = display_relative(&root, &abs);
    let source = read_utf8_file(&abs, MAX_READ_BYTES)?;
    let lang = resolve_language(input.lang.as_deref(), &abs)?;
    if lang != SupportLang::Vue {
        let _grammar = lang.grammar().load()?;
    }

    let mut allowed: Option<Vec<SymbolType>> = None;
    if let Some(ref raw_types) = input.types
        && !raw_types.is_empty()
    {
        let mut parsed = Vec::with_capacity(raw_types.len());
        for raw in raw_types {
            let sym = SymbolType::parse(raw).with_context(|| {
                format!(
                    "unsupported symbol type '{raw}'; valid types: {}",
                    SymbolType::ORDER
                        .iter()
                        .map(|t| t.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            parsed.push(sym);
        }
        allowed = Some(parsed);
    }

    let include_imports = input.include_imports.unwrap_or(false);
    let raw_items = outline::outline(&source, lang);
    let items: Vec<Value> = raw_items
        .into_iter()
        .filter(|item| include_imports || !item.is_import)
        .filter(|item| {
            allowed
                .as_ref()
                .is_none_or(|list| list.contains(&item.entry.symbol_type))
        })
        .map(format_outline_item)
        .collect();

    let mut out = json!({
        "path": display,
        "language": lang.canonical_name(),
        "total_lines": source.lines().count(),
        "count": items.len(),
        "items": items,
    });
    if lang == SupportLang::Vue {
        let sections: Vec<Value> = parse_sfc_blocks(&source)
            .into_iter()
            .map(|b| {
                json!({
                    "tag": b.tag_name,
                    "setup": b.is_setup,
                    "lang": b.lang,
                    "start_line": b.outer_start_line,
                    "end_line": b.outer_end_line,
                })
            })
            .collect();
        out["sections"] = Value::Array(sections);
    }
    Ok(out)
}

fn format_outline_item(item: Item) -> Value {
    let members: Vec<Value> = item
        .members
        .into_iter()
        .map(|m| {
            json!({
                "name": m.entry.name,
                "kind": m.entry.symbol_type.name(),
                "start_line": m.entry.range.start.line + 1,
                "end_line": m.entry.range.end.line + 1,
                "signature": m.entry.signature,
                "public": m.is_public,
            })
        })
        .collect();
    let mut obj = json!({
        "name": item.entry.name,
        "kind": item.entry.symbol_type.name(),
        "start_line": item.entry.range.start.line + 1,
        "end_line": item.entry.range.end.line + 1,
        "signature": item.entry.signature,
        "exported": item.is_exported,
    });
    if !members.is_empty() {
        obj["members"] = Value::Array(members);
    }
    obj
}

/// Run full-repository parallel AST structural search.
pub async fn ast_grep(input: AstGrepInput) -> Result<Value> {
    ensure!(
        !input.patterns.is_empty() && input.patterns.iter().all(|p| !p.trim().is_empty()),
        "provide at least one non-empty AST pattern"
    );
    let limit = input.limit.unwrap_or(30);
    ensure!((1..=100).contains(&limit), "limit must be between 1 and 100");
    let offset = input.offset.unwrap_or(0);

    let root = resolve_root(&input.root)?;
    let scope = match &input.path {
        Some(p) => resolve_in_root(&root, p)?,
        None => root.clone(),
    };
    let lang = match input.lang.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(alias) => Some(resolve_supported_lang(alias)?),
        None => None,
    };
    let strictness = parse_strictness(input.strictness.as_deref())?;
    let discover_opts = build_discover(
        lang,
        input.include.as_deref(),
        input.exclude.as_deref(),
        input.hidden.unwrap_or(false),
    )?;

    let fs = Fs::native();
    let mut candidates = discover(&fs, &scope, &discover_opts)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for c in &mut candidates {
        c.display = display_relative(&root, &c.path);
    }

    let query = Query {
        patterns: &input.patterns,
        lang: lang.map(|l| l.canonical_name()),
        selector: input.selector.as_deref(),
        strictness,
        captures: true,
    };
    let page = Page { offset, limit };
    let result = find_page(&fs, candidates, &query, page)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let matches: Vec<Value> = result
        .hits
        .iter()
        .map(|hit| format_match(&hit.display, &hit.matched))
        .collect();
    let next_offset = if result.limit_reached {
        Some(offset.saturating_add(matches.len()))
    } else {
        None
    };

    Ok(json!({
        "scope": display_relative(&root, &scope),
        "files_searched": result.report.files_searched,
        "files_with_matches": result.files_with_matches,
        "total_matches": result.total_matches,
        "offset": offset,
        "limit": limit,
        "next_offset": next_offset,
        "matches": matches,
        "warnings": result.report.warnings,
    }))
}

/// Run full-repository parallel AST rewrite (preview unified diff by default, atomic write on `apply: true`).
pub async fn ast_edit(input: AstEditInput) -> Result<Value> {
    ensure!(!input.rules.is_empty(), "provide at least one rewrite rule");
    for rule in &input.rules {
        ensure!(!rule.pattern.trim().is_empty(), "AST pattern must not be empty");
    }

    let root = resolve_root(&input.root)?;
    let scope = match &input.path {
        Some(p) => resolve_in_root(&root, p)?,
        None => root.clone(),
    };
    let lang = match input.lang.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(alias) => Some(resolve_supported_lang(alias)?),
        None => None,
    };
    let strictness = parse_strictness(input.strictness.as_deref())?;
    let discover_opts = build_discover(
        lang,
        input.include.as_deref(),
        input.exclude.as_deref(),
        false,
    )?;

    let fs = Fs::native();
    let mut candidates = discover(&fs, &scope, &discover_opts)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for c in &mut candidates {
        c.display = display_relative(&root, &c.path);
    }

    let rule_pairs: Vec<(String, String)> = input
        .rules
        .into_iter()
        .map(|r| (r.pattern, r.rewrite))
        .collect();
    let rewrite_opts = Rewrite {
        rules: &rule_pairs,
        lang: lang.map(|l| l.canonical_name()),
        selector: input.selector.as_deref(),
        strictness,
        max_replacements: input.max_replacements.unwrap_or(500),
        max_files: input.max_files.unwrap_or(100),
        fail_on_parse_error: false,
        skip_overlaps: input.skip_overlaps.unwrap_or(false),
        // Always render in memory first so we can verify syntax and preserve line endings before writing.
        mode: Mode::Render,
    };

    let edited = rewrite(&fs, candidates, &rewrite_opts)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let apply = input.apply.unwrap_or(false);
    let mut staged: Vec<(PathBuf, String, String, String, usize)> = Vec::new();
    let mut total_replacements = 0usize;

    for file in edited.files {
        let Some(rendered) = file.content else {
            continue;
        };
        let after = preserve_line_endings(&file.source, rendered);
        if after == file.source {
            continue;
        }
        // Mandatory post-rewrite syntax validation using tree-sitter.
        let grammar = file.language.grammar().load()?;
        let tree = grammar::parse(&grammar, &after, None)
            .with_context(|| format!("parse rewritten {}", file.display))?;
        ensure!(
            tree.is_some_and(|t| !t.root_node().has_error()),
            "AST rewrite would introduce a syntax error in {}; aborted without writing",
            file.display
        );
        total_replacements += file.changes.len();
        staged.push((file.path, file.display, file.source, after, file.changes.len()));
    }

    if apply {
        for (path, _, _, after, _) in &staged {
            write_atomic(path, after.as_bytes())?;
        }
    }

    let mut diff = String::new();
    let mut diff_truncated = false;
    let mut file_summaries = Vec::with_capacity(staged.len());
    for (_, display, before, after, count) in &staged {
        file_summaries.push(json!({
            "path": display,
            "replacements": count,
        }));
        if !diff_truncated {
            let chunk = unified_diff(display, before, after);
            if diff.len() + chunk.len() <= MAX_DIFF_PREVIEW_BYTES {
                diff.push_str(&chunk);
            } else {
                let remaining = MAX_DIFF_PREVIEW_BYTES.saturating_sub(diff.len());
                let cut = chunk.floor_char_boundary(remaining);
                diff.push_str(&chunk[..cut]);
                diff.push_str("\n... [diff truncated]\n");
                diff_truncated = true;
            }
        }
    }

    Ok(json!({
        "applied": apply,
        "files_searched": edited.report.files_searched,
        "files_changed": staged.len(),
        "total_replacements": total_replacements,
        "limit_reached": edited.limit_reached,
        "files": file_summaries,
        "diff": diff,
        "diff_truncated": diff_truncated,
        "warnings": edited.report.warnings,
    }))
}

pub fn supported_languages() -> String {
    supported_lang_list()
}

fn build_discover(
    lang: Option<omp_ast::language::SupportLang>,
    include: Option<&str>,
    exclude: Option<&[String]>,
    hidden: bool,
) -> Result<Discover> {
    let include = match include.map(str::trim).filter(|s| !s.is_empty()) {
        Some(pat) => Some(Selector::scope(pat, true).map_err(|e| anyhow::anyhow!("invalid include glob '{pat}': {e}"))?),
        None => None,
    };
    let exclude = match exclude {
        Some(list) if !list.is_empty() => {
            Selector::exclude(list).map_err(|e| anyhow::anyhow!("invalid exclude glob: {e}"))?
        }
        _ => None,
    };
    Ok(Discover {
        include,
        exclude,
        hidden,
        gitignore: true,
        node_modules: false,
        follow_links: false,
        workers: None,
        lang,
    })
}

fn parse_strictness(raw: Option<&str>) -> Result<MatchStrictness> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("smart") => Ok(MatchStrictness::Smart),
        Some("cst") => Ok(MatchStrictness::Cst),
        Some("ast") => Ok(MatchStrictness::Ast),
        Some("relaxed") => Ok(MatchStrictness::Relaxed),
        Some("signature") => Ok(MatchStrictness::Signature),
        Some(other) => bail!(
            "unsupported strictness '{other}'; choose smart, cst, ast, relaxed, or signature"
        ),
    }
}

fn format_match(path: &str, m: &Match) -> Value {
    let captures = m.captures.as_ref().map(|caps| {
        let single: serde_json::Map<String, Value> = caps
            .single
            .iter()
            .map(|(k, v)| (k.clone(), format_capture(v)))
            .collect();
        let multi: serde_json::Map<String, Value> = caps
            .multi
            .iter()
            .map(|(k, list)| {
                (
                    k.clone(),
                    Value::Array(list.iter().map(format_capture).collect()),
                )
            })
            .collect();
        json!({ "single": single, "multi": multi })
    });
    json!({
        "path": path,
        "start_line": m.span.start_pos.line + 1,
        "start_column": m.span.start_pos.column + 1,
        "end_line": m.span.end_pos.line + 1,
        "end_column": m.span.end_pos.column + 1,
        "text": clip(&m.text, MAX_MATCH_SNIPPET_BYTES),
        "captures": captures,
    })
}

fn format_capture(c: &Capture) -> Value {
    json!({
        "start_line": c.span.start_pos.line + 1,
        "end_line": c.span.end_pos.line + 1,
        "text": clip(&c.text, MAX_MATCH_SNIPPET_BYTES),
    })
}

fn clip(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let end = s.floor_char_boundary(max_bytes.saturating_sub(3));
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn full_repo_ast_grep_edit_outline_and_summary() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        fs::create_dir_all(root.join("src"))?;
        let sample = "export function greet(name: string) {\n  const a = 1;\n  const b = 2;\n  const c = 3;\n  const d = 4;\n  return oldCall(name, a + b + c + d);\n}\n";
        fs::write(root.join("src/a.ts"), sample)?;
        fs::write(root.join("src/b.ts"), "export const x = oldCall('hi', 1);\n")?;

        // 1. Summary
        let summary = code_summary(SummaryInput {
            root: root.clone(),
            file: PathBuf::from("src/a.ts"),
            lang: None,
            block_line: None,
            unfold_until_lines: Some(0),
            unfold_limit_lines: None,
            min_body_lines: Some(3),
            min_comment_lines: None,
            compact: Some(true),
        })?;
        assert!(summary.contains("elided"), "{summary}");

        // 1b. Syntactic block expansion (exact opener + enclosing fallback on closing brace)
        let block = code_summary(SummaryInput {
            root: root.clone(),
            file: PathBuf::from("src/a.ts"),
            lang: None,
            block_line: Some(1),
            unfold_until_lines: None,
            unfold_limit_lines: None,
            min_body_lines: None,
            min_comment_lines: None,
            compact: Some(true),
        })?;
        assert!(block.contains("L1-L7"), "{block}");
        assert!(block.contains("return oldCall"), "{block}");

        let enclosing = code_summary(SummaryInput {
            root: root.clone(),
            file: PathBuf::from("src/a.ts"),
            lang: None,
            block_line: Some(7),
            unfold_until_lines: None,
            unfold_limit_lines: None,
            min_body_lines: None,
            min_comment_lines: None,
            compact: Some(false),
        })?;
        let enc_val: Value = serde_json::from_str(&enclosing)?;
        assert_eq!(enc_val["resolution"], "enclosing");
        assert_eq!(enc_val["start_line"], 1);
        assert_eq!(enc_val["end_line"], 7);

        // 2. Outline
        let outline = code_outline(OutlineInput {
            root: root.clone(),
            file: PathBuf::from("src/a.ts"),
            lang: None,
            types: Some(vec!["function".into()]),
            include_imports: Some(false),
        })?;
        assert_eq!(outline["count"], 1);
        assert_eq!(outline["items"][0]["name"], "greet");

        // 3. Full-repo ast_grep (no explicit file list!)
        let found = ast_grep(AstGrepInput {
            root: root.clone(),
            path: None,
            patterns: vec!["oldCall($$$ARGS)".into()],
            lang: None,
            selector: None,
            include: Some("src/**/*.ts".into()),
            exclude: None,
            strictness: None,
            hidden: None,
            offset: None,
            limit: None,
        })
        .await?;
        assert_eq!(found["total_matches"], 2);
        assert_eq!(found["files_with_matches"], 2);

        // 4. Dry-run ast_edit leaves disk untouched
        let preview = ast_edit(AstEditInput {
            root: root.clone(),
            path: None,
            rules: vec![RewriteRule {
                pattern: "oldCall($$$ARGS)".into(),
                rewrite: "newCall($$$ARGS)".into(),
            }],
            lang: None,
            selector: None,
            include: None,
            exclude: None,
            strictness: None,
            skip_overlaps: None,
            max_replacements: None,
            max_files: None,
            apply: Some(false),
        })
        .await?;
        assert_eq!(preview["applied"], false);
        assert_eq!(preview["files_changed"], 2);
        assert!(preview["diff"].as_str().unwrap().contains("+export const x = newCall('hi', 1);"));
        assert!(fs::read_to_string(root.join("src/b.ts"))?.contains("oldCall"));

        // 5. Apply ast_edit writes atomically
        let applied = ast_edit(AstEditInput {
            root: root.clone(),
            path: None,
            rules: vec![RewriteRule {
                pattern: "oldCall($$$ARGS)".into(),
                rewrite: "newCall($$$ARGS)".into(),
            }],
            lang: None,
            selector: None,
            include: None,
            exclude: None,
            strictness: None,
            skip_overlaps: None,
            max_replacements: None,
            max_files: None,
            apply: Some(true),
        })
        .await?;
        assert_eq!(applied["applied"], true);
        assert!(fs::read_to_string(root.join("src/b.ts"))?.contains("newCall('hi', 1)"));

        // 6. Vue SFC summary, block_line expansion, and outline (with sections)
        let vue_src = "<script setup lang=\"ts\">\nimport a from 'a';\nimport b from 'b';\nimport c from 'c';\nimport d from 'd';\n\nfunction load() {\n  const x = 1;\n  const y = 2;\n  const z = 3;\n  return x + y + z;\n}\n</script>\n<template>\n  <div class=\"card\">\n    <span>hello</span>\n  </div>\n</template>\n";
        fs::write(root.join("src/Comp.vue"), vue_src)?;
        let vue_sum = code_summary(SummaryInput {
            root: root.clone(),
            file: PathBuf::from("src/Comp.vue"),
            lang: None,
            block_line: None,
            unfold_until_lines: Some(0),
            unfold_limit_lines: None,
            min_body_lines: Some(3),
            min_comment_lines: None,
            compact: Some(true),
        })?;
        assert!(vue_sum.starts_with("# src/Comp.vue (vue,"), "{vue_sum}");
        assert!(!vue_sum.contains("syntax fallback"), "{vue_sum}");
        assert!(vue_sum.contains("elided"), "{vue_sum}");

        let vue_block = code_summary(SummaryInput {
            root: root.clone(),
            file: PathBuf::from("src/Comp.vue"),
            lang: None,
            block_line: Some(7),
            unfold_until_lines: None,
            unfold_limit_lines: None,
            min_body_lines: None,
            min_comment_lines: None,
            compact: Some(true),
        })?;
        assert!(vue_block.contains("L7-L12"), "{vue_block}");
        assert!(vue_block.contains("return x + y + z;"), "{vue_block}");

        let vue_outline = code_outline(OutlineInput {
            root: root.clone(),
            file: PathBuf::from("src/Comp.vue"),
            lang: None,
            types: None,
            include_imports: Some(false),
        })?;
        assert_eq!(vue_outline["language"], "vue");
        assert_eq!(vue_outline["items"][0]["name"], "load");
        assert_eq!(vue_outline["items"][0]["start_line"], 7);
        assert_eq!(vue_outline["sections"].as_array().unwrap().len(), 2);
        Ok(())
    }
}
