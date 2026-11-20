use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use omp_ast::language::grammar::{self, LanguageGrammar};
use omp_ast::ops::resolve_supported_lang;
use omp_ast::search::{MatchStrictness, Mode, Rewrite, rewrite_source};
use tokio_util::sync::CancellationToken;

use crate::{Change, read_project_file};

pub(crate) fn preview(
    root: &Path,
    language: &str,
    pattern: String,
    replacement: String,
    files: &[PathBuf],
) -> Result<Vec<Change>> {
    let language = resolve_supported_lang(language)?;
    let grammar = language.grammar().load()?;
    let rules = [(pattern, replacement)];
    let rewrite = Rewrite {
        rules: &rules,
        lang: None,
        selector: None,
        strictness: MatchStrictness::Smart,
        max_replacements: usize::MAX,
        max_files: usize::MAX,
        fail_on_parse_error: true,
        skip_overlaps: false,
        mode: Mode::Render,
    };
    let cancel = CancellationToken::new();
    let mut seen = HashSet::new();
    let mut changes = Vec::new();
    let mut total_matches = 0;

    for file in files {
        let (absolute, before) = read_project_file(root, file)?;
        let path = absolute.strip_prefix(root)?.to_path_buf();
        ensure!(
            seen.insert(path.clone()),
            "duplicate file: {}",
            path.display()
        );
        let edited = rewrite_source(&before, language, &rewrite, &cancel)
            .with_context(|| format!("cannot rewrite {}", path.display()))?;
        ensure!(
            edited.warnings.is_empty(),
            "{}: {}",
            path.display(),
            edited.warnings.join(", ")
        );
        total_matches += edited.changes.len();

        let mut after = edited.content;
        if before.contains("\r\n") && !has_bare_lf(&before) && has_bare_lf(&after) {
            after = after.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        if before == after {
            continue;
        }
        let tree = grammar::parse(&grammar, &after, None)
            .with_context(|| format!("cannot parse rewritten {}", path.display()))?;
        ensure!(
            tree.is_some_and(|tree| !tree.root_node().has_error()),
            "rewrite produces a syntax error in {}",
            path.display()
        );
        changes.push((
            Change {
                path,
                before,
                after,
            },
            edited.changes.len(),
        ));
    }

    for (change, count) in &changes {
        eprintln!("{}: {count} match(es)", change.path.display());
    }
    eprintln!(
        "{total_matches} match(es), {} file(s) changed",
        changes.len()
    );

    Ok(changes.into_iter().map(|(change, _)| change).collect())
}

fn has_bare_lf(source: &str) -> bool {
    source
        .split_inclusive('\n')
        .any(|line| line.ends_with('\n') && !line.ends_with("\r\n"))
}
