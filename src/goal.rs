//! Project-local goals, inspired by the separate goal and todo state in Oh My Pi v19.1.1.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::project_state::{checked_metadata, directory};

const MAX_CRITERIA: usize = 32;
const MAX_TEXT: usize = 4_096;
const MAX_STATE: u64 = 1024 * 1024;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create a goal with a fixed objective and one or more acceptance criteria.
    Init {
        id: String,
        #[arg(long)]
        objective: String,
        #[arg(long, required = true)]
        criterion: Vec<String>,
    },
    /// List goal IDs saved for this project.
    List,
    /// Read the objective, current milestone, and evidence as JSON.
    Show { id: String },
    /// Set the current milestone while preserving the objective and criteria.
    Focus { id: String, milestone: String },
    /// Attach checked evidence to a one-based acceptance criterion.
    Record {
        id: String,
        criterion: usize,
        evidence: String,
    },
    /// Withdraw evidence that is stale or no longer applies.
    ClearEvidence { id: String, criterion: usize },
}

#[derive(Deserialize, Serialize)]
struct Goal {
    schema_version: u8,
    objective: String,
    current_milestone: Option<String>,
    criteria: Vec<Criterion>,
}

#[derive(Deserialize, Serialize)]
struct Criterion {
    description: String,
    evidence: Option<String>,
}

#[derive(Serialize)]
struct Status<'a> {
    id: &'a str,
    #[serde(flatten)]
    goal: &'a Goal,
    ready_for_review: bool,
}

pub(crate) fn run(root: &Path, command: Command) -> Result<()> {
    match command {
        Command::List => write_json(&list(root)?),
        Command::Init {
            id,
            objective,
            criterion,
        } => {
            let goal = new_goal(&objective, &criterion)?;
            let (path, lock) = lock_goal(root, &id, true)?;
            ensure!(
                checked_metadata(&path)?.is_none(),
                "goal already exists: {id}"
            );
            save(&path, &goal, true)?;
            drop(lock);
            print_status(&id, &goal)
        }
        Command::Show { id } => print_status(&id, &load(&goal_path(root, &id, false)?)?),
        Command::Focus { id, milestone } => update(root, &id, |goal| {
            goal.current_milestone = Some(text(&milestone, "milestone")?);
            Ok(())
        }),
        Command::Record {
            id,
            criterion,
            evidence,
        } => update(root, &id, |goal| {
            criterion_at(goal, criterion)?.evidence = Some(text(&evidence, "evidence")?);
            Ok(())
        }),
        Command::ClearEvidence { id, criterion } => update(root, &id, |goal| {
            criterion_at(goal, criterion)?.evidence = None;
            Ok(())
        }),
    }
}

fn new_goal(objective: &str, descriptions: &[String]) -> Result<Goal> {
    ensure!(
        (1..=MAX_CRITERIA).contains(&descriptions.len()),
        "provide 1 to {MAX_CRITERIA} acceptance criteria"
    );
    let criteria = descriptions
        .iter()
        .map(|value| {
            Ok(Criterion {
                description: text(value, "criterion")?,
                evidence: None,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Goal {
        schema_version: 1,
        objective: text(objective, "objective")?,
        current_milestone: None,
        criteria,
    })
}

fn criterion_at(goal: &mut Goal, number: usize) -> Result<&mut Criterion> {
    let len = goal.criteria.len();
    ensure!(
        (1..=len).contains(&number),
        "criterion number must be between 1 and {len}"
    );
    Ok(&mut goal.criteria[number - 1])
}

fn text(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    ensure!(
        !value.is_empty() && value.len() <= MAX_TEXT,
        "{label} must contain 1 to {MAX_TEXT} UTF-8 bytes"
    );
    ensure!(
        !value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')),
        "{label} contains an unsupported control character"
    );
    Ok(value.to_owned())
}

fn valid_id(id: &str) -> bool {
    (1..=48).contains(&id.len())
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-')
}

fn goal_path(root: &Path, id: &str, create: bool) -> Result<PathBuf> {
    ensure!(
        valid_id(id),
        "goal ID must use lowercase letters, digits, or interior hyphens"
    );
    let dir = directory(root, "goals", &[&format!("goal-{id}")], create)?
        .with_context(|| format!("goal not found: {id}"))?;
    Ok(dir.join("goal.json"))
}

fn list(root: &Path) -> Result<Vec<String>> {
    let Some(dir) = directory(root, "goals", &[], false)? else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(id) = name.to_str().and_then(|name| name.strip_prefix("goal-"))
            && valid_id(id)
            && entry.file_type()?.is_dir()
            && checked_metadata(&entry.path().join("goal.json"))?
                .is_some_and(|metadata| metadata.is_file())
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

fn lock_goal(root: &Path, id: &str, create: bool) -> Result<(PathBuf, File)> {
    let path = goal_path(root, id, create)?;
    let lock_path = path.with_file_name("state.lock");
    ensure!(
        checked_metadata(&lock_path)?.is_none_or(|metadata| metadata.is_file()),
        "expected a regular goal lock file"
    );
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let lock = options.open(lock_path).context("open goal lock")?;
    ensure!(
        lock.metadata()?.is_file(),
        "expected a regular goal lock file"
    );
    lock.lock().context("lock goal state")?;
    Ok((path, lock))
}

fn update(root: &Path, id: &str, edit: impl FnOnce(&mut Goal) -> Result<()>) -> Result<()> {
    let (path, lock) = lock_goal(root, id, false)?;
    let mut goal = load(&path)?;
    edit(&mut goal)?;
    save(&path, &goal, false)?;
    drop(lock);
    print_status(id, &goal)
}

fn load(path: &Path) -> Result<Goal> {
    ensure!(
        checked_metadata(path)?.is_some_and(|metadata| metadata.is_file()),
        "goal record is missing or is not a regular file"
    );
    let mut input = Vec::new();
    File::open(path)?
        .take(MAX_STATE + 1)
        .read_to_end(&mut input)?;
    ensure!(input.len() as u64 <= MAX_STATE, "goal record is too large");
    let goal: Goal = serde_json::from_slice(&input)
        .with_context(|| format!("read goal state {}", path.display()))?;
    ensure!(
        goal.schema_version == 1 && (1..=MAX_CRITERIA).contains(&goal.criteria.len()),
        "unsupported goal state"
    );
    Ok(goal)
}

fn save(path: &Path, goal: &Goal, create: bool) -> Result<()> {
    let mut temporary = NamedTempFile::new_in(path.parent().context("resolve goal directory")?)?;
    serde_json::to_writer_pretty(&mut temporary, goal)?;
    writeln!(temporary)?;
    temporary.as_file().sync_all()?;
    if create {
        temporary.persist_noclobber(path)
    } else {
        temporary.persist(path)
    }
    .with_context(|| format!("save goal state {}", path.display()))?;
    Ok(())
}

fn status<'a>(id: &'a str, goal: &'a Goal) -> Status<'a> {
    Status {
        id,
        goal,
        ready_for_review: goal.criteria.iter().all(|criterion| {
            criterion
                .evidence
                .as_ref()
                .is_some_and(|evidence| !evidence.trim().is_empty())
        }),
    }
}

fn print_status(id: &str, goal: &Goal) -> Result<()> {
    write_json(&status(id, goal))
}

fn write_json(value: &impl Serialize) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}
