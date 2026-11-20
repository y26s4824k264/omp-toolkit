use std::{
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::{collections::HashSet, os::unix::fs::MetadataExt};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};

mod discovery;
mod hashline;
mod structural;

#[derive(Parser)]
#[command(
    version,
    about = "Inspect source and preview edits with OMP's native engines"
)]
struct Cli {
    /// Project root. File arguments and patch paths are relative to this directory.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: EditCommand,
}

#[derive(Subcommand)]
enum EditCommand {
    /// Read numbered source lines and their OMP Hashline tag.
    Read {
        file: PathBuf,
        #[arg(long, default_value_t = 1)]
        start: usize,
        #[arg(long)]
        end: Option<usize>,
    },
    /// Preview a Hashline patch for the listed files without modifying them.
    Hashline {
        #[arg(long)]
        patch: PathBuf,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Preview one structural rewrite for the listed files without modifying them.
    Ast {
        #[arg(long)]
        lang: String,
        #[arg(long)]
        pattern: String,
        #[arg(long)]
        rewrite: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Find AST matches, with source positions and captures.
    AstFind {
        #[arg(long)]
        lang: String,
        #[arg(long)]
        pattern: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Read a source summary with original line ranges.
    ReadSummary { file: PathBuf },
}

pub(crate) struct Change {
    path: PathBuf,
    before: String,
    after: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.canonicalize().context("resolve project root")?;
    ensure!(root.is_dir(), "project root must be a directory");
    let changes = match cli.command {
        EditCommand::Read { file, start, end } => return hashline::read(&root, &file, start, end),
        EditCommand::Hashline { patch, files } => {
            let patch = fs::read_to_string(&patch)
                .with_context(|| format!("read patch {}", patch.display()))?;
            hashline::preview(&root, &patch, &files)?
        }
        EditCommand::Ast {
            lang,
            pattern,
            rewrite,
            files,
        } => structural::preview(&root, &lang, pattern, rewrite, &files)?,
        EditCommand::AstFind {
            lang,
            pattern,
            offset,
            limit,
            files,
        } => {
            return discovery::ast_find(&root, &lang, &pattern, &files, offset, limit);
        }
        EditCommand::ReadSummary { file } => return discovery::read_summary(&root, &file),
    };
    emit_patch(&root, changes)
}

pub(crate) fn read_project_file(root: &Path, path: &Path) -> Result<(PathBuf, String)> {
    let mut absolute = root.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(name) if !name.eq_ignore_ascii_case(".git") => absolute.push(name),
            _ => bail!(
                "expected a project-relative source path: {}",
                path.display()
            ),
        }
        let metadata = absolute
            .symlink_metadata()
            .with_context(|| format!("inspect {}", absolute.display()))?;
        ensure!(
            !metadata.is_symlink(),
            "symbolic links are excluded: {}",
            absolute.display()
        );
    }
    ensure!(
        absolute.is_file(),
        "expected an existing file: {}",
        path.display()
    );
    let source = fs::read_to_string(&absolute)
        .with_context(|| format!("read UTF-8 source {}", path.display()))?;
    ensure!(
        !source.contains('\0'),
        "expected text source: {}",
        path.display()
    );
    Ok((absolute, source))
}

fn emit_patch(root: &Path, mut changes: Vec<Change>) -> Result<()> {
    changes.retain(|change| change.before != change.after);
    if changes.is_empty() {
        return Ok(());
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    ensure!(
        changes.windows(2).all(|pair| pair[0].path != pair[1].path),
        "duplicate file in edit batch"
    );

    let snapshot = tempfile::tempdir().context("create diff workspace")?;
    #[cfg(unix)]
    let mut identities = HashSet::new();
    for change in changes {
        let (absolute, current) = read_project_file(root, &change.path)?;
        #[cfg(unix)]
        {
            let metadata = absolute.metadata()?;
            ensure!(
                identities.insert((metadata.dev(), metadata.ino())),
                "more than one path refers to the same file: {}",
                change.path.display()
            );
        }
        ensure!(
            current == change.before,
            "source changed during preview: {}",
            change.path.display()
        );
        write_diff_input(
            &snapshot.path().join("a").join(&change.path),
            &change.before,
        )?;
        write_diff_input(&snapshot.path().join("b").join(&change.path), &change.after)?;
    }

    let diff = Command::new("git")
        .current_dir(snapshot.path())
        .args([
            "diff",
            "--no-index",
            "--no-prefix",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--binary",
            "--",
            "a",
            "b",
        ])
        .output()
        .context("run git diff")?;
    ensure!(
        diff.status.success() || diff.status.code() == Some(1),
        "git diff failed: {}",
        String::from_utf8_lossy(&diff.stderr).trim()
    );
    io::stdout()
        .lock()
        .write_all(&diff.stdout)
        .context("write Git patch")
}

fn write_diff_input(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().context("resolve diff input directory")?;
    fs::create_dir_all(parent)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create diff input {}", path.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("write diff input {}", path.display()))
}
