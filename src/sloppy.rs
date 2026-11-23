use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use omp_edit::{
    EditStore,
    modes::sloppy::{
        apply::{Applied, ApplyContext, apply_sloppy},
        parse::split_sloppy_sections,
    },
    notebook::is_notebook_path,
    text::{LineEnding, detect_line_ending, normalize_to_lf, restore_line_endings, strip_bom},
};

use crate::{Change, edit_path_policy, read_project_file};

pub(crate) fn preview(root: &Path, instructions: &str, files: &[PathBuf]) -> Result<Vec<Change>> {
    ensure!(
        !files.is_empty(),
        "specify each file named in the Sloppy patch"
    );
    let mut declared: BTreeSet<_> = files.iter().map(PathBuf::as_path).collect();
    ensure!(
        declared.len() == files.len(),
        "duplicate Sloppy source path"
    );

    let sections = split_sloppy_sections(instructions)?;
    ensure!(
        !sections.is_empty(),
        "start the Sloppy patch with *** Edit File: relative/path"
    );
    let policy = edit_path_policy(root);
    let store = EditStore::new();
    let mut changes = Vec::with_capacity(sections.len());
    let mut diagnostics = Vec::new();

    for section in sections {
        let target = Path::new(&section.path);
        ensure!(
            declared.remove(target),
            "Sloppy patch targets an undeclared or repeated file: {}",
            target.display()
        );
        let (absolute, before) = read_project_file(root, target)?;
        let path = absolute
            .strip_prefix(root)
            .context("Sloppy source outside the project")?
            .to_owned();
        let display = path
            .to_str()
            .context("Sloppy requires a UTF-8 source path")?;
        ensure!(
            !is_notebook_path(&path),
            "Sloppy preview requires a text source file: {}",
            path.display()
        );
        if let Some(message) = policy.auto_generated_message(display, before.as_bytes()) {
            bail!("{message}");
        }

        let (bom, body) = strip_bom(&before);
        let ending = detect_line_ending(body);
        let uses_crlf = ending == LineEnding::CrLf;
        ensure!(
            body.split_inclusive('\n')
                .all(|line| !line.ends_with('\n') || line.ends_with("\r\n") == uses_crlf),
            "cannot preserve mixed LF and CRLF line endings in Sloppy preview for {}",
            path.display()
        );
        let mut notes = Vec::new();
        let applied = apply_sloppy(
            &normalize_to_lf(body),
            &section.body,
            ApplyContext {
                path: display,
                notes: &mut notes,
                store: &store,
                canonical: &absolute,
                streaming: false,
            },
        )
        .with_context(|| format!("preview Sloppy edits for {}", path.display()))?;
        let edited = match applied {
            Applied::Changed {
                text,
                rejected: None,
            } => text,
            Applied::Changed {
                rejected: Some(error),
                ..
            }
            | Applied::Unchanged(error) => {
                return Err(error)
                    .with_context(|| format!("preview Sloppy edits for {}", path.display()));
            }
        };
        let mut after = bom.to_owned();
        after.push_str(&restore_line_endings(&edited, ending));
        diagnostics.extend(
            notes
                .into_iter()
                .map(|note| format!("{}: {note}", path.display())),
        );
        changes.push(Change {
            path,
            before,
            after,
        });
    }

    if let Some(path) = declared.first() {
        bail!("the Sloppy patch has no section for {}", path.display());
    }
    for note in diagnostics {
        eprintln!("{note}");
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn previews_a_block_replacement_and_preserves_bom_and_crlf() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let name = "queue.ts";
        let file = directory.path().join(name);
        let before = "\u{feff}export function drain() {\r\n  return 1;\r\n}\r\n";
        fs::write(&file, before)?;

        let changes = preview(
            directory.path(),
            "*** Edit File: queue.ts\n*** Replace Block: export function drain(\nexport function drain() {\n  return 2;\n}\n",
            &[name.into()],
        )?;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, Path::new(name));
        assert_eq!(changes[0].before, before);
        assert_eq!(
            changes[0].after,
            "\u{feff}export function drain() {\r\n  return 2;\r\n}\r\n"
        );
        assert_eq!(fs::read_to_string(file)?, before);
        Ok(())
    }

    #[test]
    fn preview_rejects_a_partially_successful_edit() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let name = "lines.txt";
        let file = directory.path().join(name);
        let before = "one\ntwo\n";
        fs::write(&file, before)?;
        let error = preview(
            directory.path(),
            "*** Edit File: lines.txt\n*** Find\none\n*** Replace\nchanged\n*** Find\nmissing anchor\n*** Replace\nshould never appear\n",
            &[name.into()],
        )
        .err()
        .context("the rejected operation should invalidate the entire preview")?;
        assert!(format!("{error:#}").contains("did not match"));
        assert_eq!(fs::read_to_string(file)?, before);
        Ok(())
    }

    #[test]
    fn rejects_a_file_missing_from_the_allowlist() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let error = preview(
            directory.path(),
            "*** Edit File: ../outside.txt\n*** Find\none\n*** Replace\ntwo\n",
            &["inside.txt".into()],
        )
        .err()
        .context("a patch should never read an undeclared file")?;
        assert!(format!("{error:#}").contains("undeclared or repeated file"));
        Ok(())
    }

    #[test]
    fn rejects_a_generated_file_even_with_an_aliased_path() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("model.pb.go"), "package original\n")?;
        let error = preview(
            directory.path(),
            "*** Edit File: model.pb.go/\n*** Find\npackage original\n*** Replace\npackage changed\n",
            &["model.pb.go/".into()],
        )
        .err()
        .context("a trailing separator must not bypass the generated-file guard")?;
        assert!(format!("{error:#}").contains("Cannot modify auto-generated file"));
        Ok(())
    }

    #[test]
    fn rejects_mixed_newlines_instead_of_rewriting_untouched_lines() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("lines.txt");
        let before = "keep-a\r\nold\r\nkeep-b\n";
        fs::write(&path, before)?;
        let error = preview(
            directory.path(),
            "*** Edit File: lines.txt\n*** Find\nold\n*** Replace\nnew\n",
            &["lines.txt".into()],
        )
        .err()
        .context("normalizing mixed newlines would affect unrelated lines")?;
        assert!(format!("{error:#}").contains("mixed LF and CRLF"));
        assert_eq!(fs::read_to_string(path)?, before);
        Ok(())
    }
}
