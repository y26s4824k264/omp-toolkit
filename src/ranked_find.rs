//! Offline lexical ranking adapted from Oh My Pi v19.0.0's
//! `packages/coding-agent/src/tools/jfind/lexical.ts` under the MIT license.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashSet},
    io::{self, Write},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use omp_xfs::{
    Fs,
    grep::{self, GrepOptions},
    walk::Stop,
};
use regex::RegexBuilder;
use serde::Serialize;

const MAX_QUERY_BYTES: usize = 1_024;
const MAX_TERMS: usize = 12;
const MAX_RESULTS: usize = 24;
const MAX_MATCHES: u64 = 8_192;
const MAX_MATCHES_PER_FILE: u64 = 64;
const MAX_LINE_BYTES: usize = 240;
const MAX_EVIDENCE_PER_FILE: usize = 3;
const SCAN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize)]
struct Evidence {
    line: u64,
    text: String,
    column: Option<u32>,
    truncated: bool,
}

struct Candidate {
    counts: Vec<usize>,
    evidence: Vec<(usize, Evidence)>,
}

#[derive(Serialize)]
struct RankedFile {
    path: String,
    matched_terms: Vec<String>,
    score: f64,
    evidence: Vec<Evidence>,
}

#[derive(Serialize)]
struct RankedResults {
    query_terms: Vec<String>,
    files_searched: u64,
    scan_truncated: bool,
    skipped_oversized: u64,
    results_truncated: bool,
    results: Vec<RankedFile>,
}

pub(crate) fn ranked_find(root: &Path, query: &str, limit: usize) -> Result<()> {
    let results = rank(root, query, limit)?;
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &results)?;
    writeln!(out)?;
    Ok(())
}

fn rank(root: &Path, query: &str, limit: usize) -> Result<RankedResults> {
    ensure!(
        query.len() <= MAX_QUERY_BYTES,
        "ranked-find query can contain at most {MAX_QUERY_BYTES} bytes"
    );
    ensure!(
        (1..=MAX_RESULTS).contains(&limit),
        "ranked-find limit must be between 1 and {MAX_RESULTS}"
    );
    let mut seen = HashSet::new();
    let terms: Vec<String> = query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .filter(|term| seen.insert(term.clone()))
        .collect();
    ensure!(
        (1..=MAX_TERMS).contains(&terms.len()),
        "ranked-find query must contain 1 to {MAX_TERMS} distinct terms"
    );
    let patterns: Vec<_> = terms.iter().map(|term| regex::escape(term)).collect();
    let matchers = patterns
        .iter()
        .map(|pattern| RegexBuilder::new(pattern).case_insensitive(true).build())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut options =
        GrepOptions::compile(patterns.join("|"), true, false).map_err(anyhow::Error::msg)?;
    // Always walk local files instead of forwarding to an ovnfs daemon.
    options.source = None;
    options.walk.hidden = false;
    options.walk.gitignore = true;
    options.walk.follow_links = false;
    options.max_columns = Some(MAX_LINE_BYTES);
    options.max_count = Some(MAX_MATCHES);
    options.max_count_per_file = Some(MAX_MATCHES_PER_FILE);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("start ranked-find search runtime")?;
    // Drive the deadline separately so it can stop a walker even when all
    // search workers are busy with synchronous filesystem calls.
    let controller = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .context("start ranked-find timeout controller")?;
    let stop = Stop::new();
    let root = root.to_path_buf();
    let worker_stop = stop.clone();
    let work = runtime.spawn(async move {
        worker_stop
            .scope(grep::grep(&Fs::native(), &root, options))
            .await
    });
    let abort = work.abort_handle();
    let scan = controller.block_on(async { tokio::time::timeout(SCAN_TIMEOUT, work).await });
    let scan = match scan {
        Ok(result) => result.context("ranked-find search task failed")??,
        Err(error) => {
            stop.stop();
            abort.abort();
            runtime.shutdown_timeout(Duration::from_millis(250));
            return Err(error)
                .context("ranked-find exceeded the 5-second scan budget; pass a narrower --root");
        }
    };

    let mut files: BTreeMap<String, Candidate> = BTreeMap::new();
    for hit in scan.matches {
        let file = files.entry(hit.path).or_insert_with(|| Candidate {
            counts: vec![0; terms.len()],
            evidence: Vec::new(),
        });
        let mut coverage = 0;
        for (index, matcher) in matchers.iter().enumerate() {
            let count = matcher.find_iter(&hit.text).count();
            coverage += usize::from(count > 0);
            file.counts[index] += count;
        }
        file.evidence.push((
            coverage,
            Evidence {
                line: hit.line,
                text: hit.text,
                column: hit.column,
                truncated: hit.truncated,
            },
        ));
    }

    let weights: Vec<_> = (0..terms.len())
        .map(|index| {
            let document_frequency = files.values().filter(|file| file.counts[index] > 0).count();
            ((scan.files_searched as f64 + 1.0) / (document_frequency as f64 + 1.0))
                .ln()
                .clamp(0.5, 6.0)
        })
        .collect();
    let mut results: Vec<_> = files
        .into_iter()
        .map(|(path, mut candidate)| {
            let mut score = 0.0;
            let mut matched_terms = Vec::new();
            for (index, term) in terms.iter().enumerate() {
                let in_path = matchers[index].is_match(&path);
                let count = candidate.counts[index];
                if count > 0 || in_path {
                    matched_terms.push(term.clone());
                }
                let path_bonus = if in_path { 2.0 } else { 0.0 };
                score += weights[index] * (path_bonus + (count as f64).ln_1p());
            }
            candidate
                .evidence
                .sort_unstable_by_key(|(coverage, item)| (Reverse(*coverage), item.line));
            RankedFile {
                path,
                matched_terms,
                score,
                evidence: candidate
                    .evidence
                    .into_iter()
                    .take(MAX_EVIDENCE_PER_FILE)
                    .map(|(_, item)| item)
                    .collect(),
            }
        })
        .collect();
    results.sort_unstable_by(|left, right| {
        right
            .matched_terms
            .len()
            .cmp(&left.matched_terms.len())
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.path.cmp(&right.path))
    });
    let results_truncated = results.len() > limit;
    results.truncate(limit);
    Ok(RankedResults {
        query_terms: terms,
        files_searched: scan.files_searched,
        scan_truncated: scan.limit_reached,
        skipped_oversized: scan.skipped_oversized,
        results_truncated,
        results,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn ranks_lexical_coverage_and_evidence_without_reading_ignored_or_binary_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        fs::create_dir(root.join(".git"))?;
        fs::write(root.join(".gitignore"), "ignored.txt\n")?;
        fs::write(root.join("ignored.txt"), "payment retry payment retry\n")?;
        fs::write(root.join(".hidden.txt"), "payment retry\n")?;
        fs::write(root.join("object.bin"), b"payment retry\0raw")?;
        fs::write(root.join("a.txt"), "payment payment payment payment\n")?;
        fs::write(root.join("b.txt"), "retry later\nPayment and retry\n")?;
        let found = rank(root, "Payment retry payment", 1)?;
        assert_eq!(found.query_terms, ["payment", "retry"]);
        assert!(found.results_truncated);
        assert!(!found.scan_truncated);
        assert_eq!(found.results[0].path, "b.txt");
        assert_eq!(found.results[0].evidence[0].line, 2);
        assert_eq!(
            rank(root, "payment retry", 10)?
                .results
                .into_iter()
                .map(|entry| entry.path)
                .collect::<Vec<_>>(),
            ["b.txt", "a.txt"],
        );
        Ok(())
    }
}
