use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use omp_edit::{
    EditError, EditResult, EditStore, FileOp, ModeEngine, PathPolicy, Resolved,
    files::{FileRead, FileSource},
    modes::hashline::{
        HashlineEngine,
        format::{
            compute_file_hash, format_hashline_header, format_numbered_line,
            split_addressable_file_lines,
        },
        input::{Patch, SplitOptions},
    },
    notebook::is_notebook_path,
    stream_json::snapshot_from_text,
    text::{detect_line_ending, normalize_to_lf, strip_bom},
};

use crate::{Change, edit_path_policy, read_project_file};

pub(crate) fn read(root: &Path, file: &Path, start: usize, end: Option<usize>) -> Result<()> {
    let (absolute, raw) = read_project_file(root, file)?;
    let display = relative_name(root, &absolute)?;
    let text = normalize_to_lf(strip_bom(&raw).1);
    let lines = split_addressable_file_lines(&text);
    let end = end.unwrap_or(lines.len());
    ensure!(
        start > 0 && start <= end && end <= lines.len(),
        "expected a line range within 1..={} for {display}",
        lines.len()
    );
    let start_number = u32::try_from(start).context("first line exceeds the hashline range")?;
    let end_number = u32::try_from(end).context("last line exceeds the hashline range")?;

    println!(
        "{}",
        format_hashline_header(display, &compute_file_hash(&text))
    );
    for (number, line) in (start_number..=end_number).zip(&lines[start - 1..end]) {
        println!("{}", format_numbered_line(number, line));
    }
    Ok(())
}

pub(crate) fn preview(root: &Path, patch: &str, files: &[PathBuf]) -> Result<Vec<Change>> {
    ensure!(
        !files.is_empty(),
        "specify each file named in the hashline patch"
    );
    let mut source = Snapshots::new(root);
    let store = EditStore::new();
    let mut expected_headers = BTreeSet::new();
    for file in files {
        let (absolute, raw) = read_project_file(root, file)?;
        let display = relative_name(root, &absolute)?.to_owned();
        ensure!(
            !source.files.contains_key(&display),
            "duplicate hashline source: {display}"
        );
        let (bom, body) = strip_bom(&raw);
        let ending = detect_line_ending(body);
        let text = normalize_to_lf(body).into_owned();
        if let Some(message) = source
            .policy
            .auto_generated_message(&display, &raw.as_bytes()[..raw.len().min(1024)])
        {
            bail!("{message}");
        }
        let tag = store.record(&absolute, &text, None);
        expected_headers.insert(format_hashline_header(&display, &tag));
        let resolved = Resolved {
            absolute: absolute.clone(),
            display: display.clone(),
        };
        source.files.insert(
            display,
            Arc::new(FileRead {
                canonical: absolute,
                resolved,
                raw,
                bom,
                ending,
                text,
                is_notebook: false,
            }),
        );
    }

    for header in patch
        .lines()
        .filter(|line| line.trim_start().starts_with('['))
    {
        ensure!(
            expected_headers.remove(header),
            "unexpected, repeated, or stale hashline header: {header}; read the file again"
        );
    }
    ensure!(
        expected_headers.is_empty(),
        "the patch must include a header for each declared file"
    );
    let parsed = Patch::parse(
        patch,
        &SplitOptions {
            cwd: Some(root),
            path: None,
        },
    )?;
    ensure!(
        parsed.sections.len() == files.len(),
        "the patch must contain one edit section per declared file"
    );
    for section in &parsed.sections {
        let read = source.file(&section.path)?;
        ensure!(
            section.file_hash.as_deref() == Some(compute_file_hash(&read.text).as_str()),
            "stale hashline tag for {}; read the file again",
            section.path
        );
        ensure!(
            section.file_op()?.is_none(),
            "hashline preview supports editing existing files only"
        );
    }

    let engine = HashlineEngine {
        enforce_seen_lines: false,
        max_columns: 0,
    };
    let args = snapshot_from_text(patch, true, true);
    let mut changes = Vec::new();
    for edit in engine.stage(&args, &mut source, &store)? {
        if edit.op == FileOp::Noop {
            continue;
        }
        ensure!(
            edit.op == FileOp::Update && edit.existed && edit.move_to.is_none(),
            "hashline preview supports editing existing files only"
        );
        let path = PathBuf::from(relative_name(root, &edit.absolute)?);
        for warning in edit.warnings {
            eprintln!("{}: {}", path.display(), warning);
        }
        changes.push(Change {
            path,
            before: edit
                .before_raw
                .context("hashline did not return the original source")?,
            after: edit
                .persisted
                .context("hashline did not return the edited source")?,
        });
    }
    Ok(changes)
}

fn relative_name<'a>(root: &Path, absolute: &'a Path) -> Result<&'a str> {
    let relative = absolute
        .strip_prefix(root)
        .context("hashline source outside the project")?;
    let display = relative
        .to_str()
        .context("hashline requires a UTF-8 file name")?;
    ensure!(
        !display.is_empty()
            && display == display.trim()
            && !display.contains(['#', '[', ']', '\r', '\n']),
        "file name is not representable in a hashline header: {display:?}"
    );
    ensure!(
        !is_notebook_path(absolute),
        "hashline adapter requires a text source file: {display}"
    );
    Ok(display)
}

/// Give the upstream engine precisely the immutable files validated by the caller.
struct Snapshots {
    policy: PathPolicy,
    files: BTreeMap<String, Arc<FileRead>>,
}

impl Snapshots {
    fn new(root: &Path) -> Self {
        Self {
            policy: edit_path_policy(root),
            files: BTreeMap::new(),
        }
    }

    fn file(&self, authored: &str) -> EditResult<&Arc<FileRead>> {
        self.files.get(authored).ok_or_else(|| {
            EditError::apply(format!(
                "hashline patch targets an undeclared file: {authored}"
            ))
        })
    }
}

impl FileSource for Snapshots {
    fn policy(&self) -> &PathPolicy {
        &self.policy
    }

    fn resolve(&mut self, authored: &str, _must_exist: bool) -> EditResult<Resolved> {
        Ok(self.file(authored)?.resolved.clone())
    }

    fn exists(&mut self, absolute: &Path) -> bool {
        self.files.values().any(|file| file.canonical == absolute)
    }

    fn read(&mut self, authored: &str) -> EditResult<Arc<FileRead>> {
        Ok(Arc::clone(self.file(authored)?))
    }

    fn try_read(&mut self, resolved: &Resolved) -> EditResult<Option<Arc<FileRead>>> {
        let file = self.file(&resolved.display)?;
        if file.canonical != resolved.absolute {
            return Err(EditError::apply(
                "hashline attempted to change the source path",
            ));
        }
        Ok(Some(Arc::clone(file)))
    }

    fn clear(&mut self) {}
}
