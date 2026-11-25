//! Project-local learning artifacts, inspired by OMP v19.1.0's MIT learn tool
//! and managed-skills writer (a7135b8f31826c878734388b648f52bed8da436e).

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::project_state::{checked_metadata, directory};

const MAX_LESSON_CHARS: usize = 2_000;
const MAX_CONTEXT_CHARS: usize = 400;
const MAX_LESSON_BYTES: usize = 16_384;
const MAX_LISTED_LESSONS: usize = 100;
const MAX_DESCRIPTION_CHARS: usize = 1_024;
const MAX_SKILL_BYTES: usize = 64_000;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Save an explicitly supplied, reusable project lesson.
    Save {
        lesson: String,
        #[arg(long)]
        context: Option<String>,
    },
    /// List up to 100 recent saved project lessons as JSON.
    List,
    /// Prepare a portable Agent Skill draft in the project learning area.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum SkillCommand {
    /// Render SKILL.md to stdout without writing files.
    Preview(SkillInput),
    /// Write a new draft. An existing draft is protected.
    Create(SkillInput),
    /// Replace an existing draft.
    Update(SkillInput),
}

#[derive(Args)]
pub(crate) struct SkillInput {
    name: String,
    #[arg(long)]
    description: String,
    /// Markdown body without YAML frontmatter. Use - to read stdin.
    #[arg(long)]
    body_file: PathBuf,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Lesson {
    lesson: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

pub(crate) fn run(root: &Path, command: Command) -> Result<()> {
    match command {
        Command::Save { lesson, context } => {
            let lesson = Lesson {
                lesson: normalize_text(&lesson, "lesson", MAX_LESSON_CHARS)?,
                context: context
                    .as_deref()
                    .map(|value| normalize_text(value, "context", MAX_CONTEXT_CHARS))
                    .transpose()?,
            };
            let (path, created) = save_lesson(root, lesson)?;
            write_json(&serde_json::json!({ "path": path, "created": created }))
        }
        Command::List => write_json(&list_lessons(root)?),
        Command::Skill { command } => {
            let (input, update) = match command {
                SkillCommand::Preview(input) => (input, None),
                SkillCommand::Create(input) => (input, Some(false)),
                SkillCommand::Update(input) => (input, Some(true)),
            };
            let name = skill_name(&input.name)?;
            let body = if input.body_file == Path::new("-") {
                read_text(io::stdin().lock(), MAX_SKILL_BYTES)?
            } else {
                let path = root.join(&input.body_file);
                read_regular_text(&path, MAX_SKILL_BYTES)
                    .with_context(|| format!("read skill body {}", path.display()))?
            };
            let content = skill_markdown(&name, &input.description, &body)?;
            if let Some(update) = update {
                let path = save_skill(root, &name, &content, update)?;
                writeln!(io::stdout().lock(), "{}", path.display())?;
            } else {
                io::stdout().lock().write_all(content.as_bytes())?;
            }
            Ok(())
        }
    }
}

fn write_json(value: &impl Serialize) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

fn normalize_text(text: &str, label: &str, limit: usize) -> Result<String> {
    let text = text.replace("\r\n", "\n");
    let text = text.trim();
    ensure!(!text.is_empty(), "{label} must not be empty");
    ensure!(
        text.chars().count() <= limit,
        "{label} must be at most {limit} characters"
    );
    ensure!(
        !text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t')),
        "{label} contains an unsupported control character"
    );
    Ok(text.to_owned())
}

fn skill_name(name: &str) -> Result<String> {
    let name = name.trim().to_ascii_lowercase();
    ensure!(
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-')
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.contains("--"),
        "skill name must use lowercase letters, digits and single hyphens between words"
    );
    let device_number = |prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'1'..=b'9']))
    };
    ensure!(
        !matches!(name.as_str(), "con" | "prn" | "aux" | "nul")
            && !device_number("com")
            && !device_number("lpt"),
        "skill name is reserved on Windows"
    );
    Ok(name)
}

fn skill_markdown(name: &str, description: &str, body: &str) -> Result<String> {
    let description = normalize_text(description, "description", MAX_DESCRIPTION_CHARS)?;
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!body.trim().is_empty(), "skill body must not be empty");
    ensure!(!body.contains('\0'), "skill body contains a NUL character");
    ensure!(
        body.lines()
            .find(|line| !line.trim().is_empty())
            .is_none_or(|line| line.trim_start_matches('\u{feff}').trim_end() != "---"),
        "skill body must omit YAML frontmatter"
    );
    // JSON quoted strings with these inputs are also valid YAML scalars.
    let name = serde_json::to_string(name)?;
    let description = serde_json::to_string(&description)?;
    let mut skill = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}");
    if !skill.ends_with('\n') {
        skill.push('\n');
    }
    ensure!(
        skill.len() <= MAX_SKILL_BYTES,
        "SKILL.md exceeds {MAX_SKILL_BYTES} bytes"
    );
    Ok(skill)
}

fn save_lesson(root: &Path, lesson: Lesson) -> Result<(PathBuf, bool)> {
    let mut encoded = serde_json::to_vec(&lesson)?;
    encoded.push(b'\n');
    // A stable FNV-1a identifier lets simultaneous saves of the same lesson coalesce.
    // Verify the content on collision so neither lesson can silently replace another.
    let id = encoded.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    let path = learning_directory(root, &["lessons"], true)?
        .expect("write creates directory")
        .join(format!("{id:016x}.json"));
    let created = write_atomic(root, &["lessons"], &path, &encoded, false)?;
    if !created {
        let existing = read_regular_text(&path, MAX_LESSON_BYTES)?;
        ensure!(
            serde_json::from_str::<Lesson>(&existing)? == lesson,
            "lesson identifier collision"
        );
    }
    Ok((path, created))
}

fn list_lessons(root: &Path) -> Result<Vec<Lesson>> {
    let Some(dir) = learning_directory(root, &["lessons"], false)? else {
        return Ok(Vec::new());
    };
    let mut newest = BTreeSet::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("list {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        if id.len() != 16 || !id.bytes().all(|ch| ch.is_ascii_hexdigit()) {
            continue;
        }
        let path = entry.path();
        let metadata = checked_metadata(&path)?.context("lesson disappeared")?;
        ensure!(
            metadata.is_file(),
            "lesson is not a regular file: {}",
            path.display()
        );
        newest.insert((metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), path));
        if newest.len() > MAX_LISTED_LESSONS {
            newest.pop_first();
        }
    }
    newest
        .into_iter()
        .rev()
        .map(|(_, path)| {
            let data = read_regular_text(&path, MAX_LESSON_BYTES)?;
            serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))
        })
        .collect()
}

fn save_skill(root: &Path, name: &str, skill: &str, update: bool) -> Result<PathBuf> {
    let components = ["skills", name];
    let Some(dir) = learning_directory(root, &components, !update)? else {
        bail!("skill draft does not exist; use learn skill create");
    };
    let path = dir.join("SKILL.md");
    ensure!(
        write_atomic(root, &components, &path, skill.as_bytes(), update)?,
        "skill draft already exists; use learn skill update"
    );
    Ok(path)
}

fn learning_directory(root: &Path, components: &[&str], create: bool) -> Result<Option<PathBuf>> {
    directory(root, "learning", components, create)
}

fn read_regular_text(path: &Path, limit: usize) -> Result<String> {
    let metadata =
        checked_metadata(path)?.with_context(|| format!("missing {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "expected a regular file at {}",
        path.display()
    );
    read_text(File::open(path)?, limit)
}

fn read_text(input: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    input.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "input exceeds {limit} bytes");
    String::from_utf8(bytes).context("input must be valid UTF-8")
}

fn write_atomic(
    root: &Path,
    components: &[&str],
    path: &Path,
    data: &[u8],
    update: bool,
) -> Result<bool> {
    let check_target = || -> Result<bool> {
        ensure!(
            learning_directory(root, components, false)?.is_some(),
            "learning directory disappeared"
        );
        let existing = checked_metadata(path)?;
        if let Some(metadata) = &existing {
            ensure!(
                metadata.is_file(),
                "expected a regular file at {}",
                path.display()
            );
        }
        ensure!(
            !update || existing.is_some(),
            "skill draft does not exist; use learn skill create"
        );
        Ok(existing.is_some())
    };
    if check_target()? && !update {
        return Ok(false);
    }
    let mut temp = NamedTempFile::new_in(path.parent().context("resolve draft parent")?)?;
    temp.write_all(data)?;
    temp.as_file().sync_all()?;
    if check_target()? && !update {
        return Ok(false);
    }
    if update {
        temp.persist(path)
    } else {
        temp.persist_noclobber(path)
    }
    .map(|_| true)
    .or_else(|err| {
        if !update && err.error.kind() == io::ErrorKind::AlreadyExists {
            return Ok(false);
        }
        Err(err)
    })
    .with_context(|| format!("save {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lesson_save_deduplicates_and_lists_with_context() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        let lesson = || Lesson {
            lesson: "先核对来源".into(),
            context: Some("发布前".into()),
        };
        assert!(list_lessons(root)?.is_empty());
        assert!(!root.join(".omp-toolkit").exists());
        let (path, created) = save_lesson(root, lesson())?;
        assert!(created);
        assert_eq!(save_lesson(root, lesson())?, (path, false));
        assert_eq!(list_lessons(root)?, [lesson()]);
        Ok(())
    }

    #[test]
    fn preview_renders_agent_skill_and_writes_require_explicit_action() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        let name = skill_name("Release-Check")?;
        let initial = skill_markdown(&name, "Use: review \"release\" steps", "Run checks.")?;
        assert_eq!(
            initial,
            "---\nname: \"release-check\"\ndescription: \"Use: review \\\"release\\\" steps\"\n---\n\nRun checks.\n"
        );
        let indented = "    cargo test\n    cargo fmt --check\n";
        assert!(skill_markdown(&name, "Use on releases", indented)?.ends_with(indented));
        let literal_delimiter = "\n    ---\n    example";
        assert!(
            skill_markdown(&name, "Use on releases", literal_delimiter)?
                .ends_with(&format!("{literal_delimiter}\n"))
        );
        assert!(!root.join(".omp-toolkit").exists());
        assert!(save_skill(root, &name, &initial, true).is_err());
        let path = save_skill(root, &name, &initial, false)?;
        assert!(save_skill(root, &name, "wrong", false).is_err());
        assert_eq!(fs::read_to_string(&path)?, initial);
        let updated = skill_markdown(&name, "Use on releases", "Run more checks.")?;
        assert_eq!(save_skill(root, &name, &updated, true)?, path);
        assert_eq!(fs::read_to_string(path)?, updated);
        assert!(skill_name("../../elsewhere").is_err());
        assert!(skill_name("COM2").is_err());
        assert!(skill_markdown(&name, "Use on releases", "---\nname: shadow").is_err());
        assert!(skill_markdown(&name, "Use on releases", &"x".repeat(MAX_SKILL_BYTES)).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn destination_does_not_follow_directory_or_file_symlinks() -> Result<()> {
        use std::os::unix::fs::symlink;
        let project = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let root = project.path();
        symlink(outside.path(), root.join(".omp-toolkit"))?;
        assert!(save_skill(root, "example", "data", false).is_err());
        assert!(fs::read_dir(outside.path())?.next().is_none());
        fs::remove_file(root.join(".omp-toolkit"))?;

        let path = save_skill(root, "example", "initial", false)?;
        let outside_file = outside.path().join("SKILL.md");
        fs::write(&outside_file, "keep")?;
        fs::remove_file(&path)?;
        symlink(&outside_file, &path)?;
        assert!(save_skill(root, "example", "overwrite", true).is_err());
        assert_eq!(fs::read_to_string(outside_file)?, "keep");
        Ok(())
    }
}
