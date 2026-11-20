use std::{
    fmt::Write as _,
    io::{self, Write},
    path::{Component, Path},
};

use omp_diff::{HunkLine, diff_line_tokens, line_tokens_str, structured_patch_hunk_rows};

use crate::Change;

pub(super) fn write_change(output: &mut impl Write, change: &Change) -> io::Result<()> {
    let old_tokens = line_tokens_str(&change.before);
    let new_tokens = line_tokens_str(&change.after);
    let runs = diff_line_tokens(&old_tokens, &new_tokens);
    let hunks = structured_patch_hunk_rows(Some(3), &old_tokens, &new_tokens, &runs);
    let old_path = quoted_path('a', &change.path);
    let new_path = quoted_path('b', &change.path);
    writeln!(output, "diff --git {old_path} {new_path}")?;
    writeln!(output, "--- {old_path}")?;
    writeln!(output, "+++ {new_path}")?;
    for hunk in hunks {
        writeln!(
            output,
            "@@ -{},{} +{},{} @@",
            range_start(hunk.old_start, hunk.old_lines),
            hunk.old_lines,
            range_start(hunk.new_start, hunk.new_lines),
            hunk.new_lines,
        )?;
        for row in hunk.lines {
            match row {
                HunkLine::Context(text) => writeln!(output, " {text}")?,
                HunkLine::Removed(text) => writeln!(output, "-{text}")?,
                HunkLine::Added(text) => writeln!(output, "+{text}")?,
                HunkLine::NoNewlineAtEof => writeln!(output, "\\ No newline at end of file")?,
            }
        }
    }
    Ok(())
}

fn range_start(first_line: u32, lines: u32) -> u32 {
    // An empty range is addressed by the line before it.
    if lines == 0 {
        first_line - 1
    } else {
        first_line
    }
}

fn quoted_path(side: char, path: &Path) -> String {
    let mut quoted = format!("\"{side}");
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        quoted.push('/');
        // Git C quoting encodes raw Unix bytes and UTF-8 filenames as octal escapes.
        for &byte in name.as_encoded_bytes() {
            match byte {
                b'"' | b'\\' => {
                    quoted.push('\\');
                    quoted.push(char::from(byte));
                }
                b' '..=b'~' => quoted.push(char::from(byte)),
                _ => write!(quoted, "\\{byte:03o}").expect("write to a String"),
            }
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process::{Command, Stdio},
    };

    use super::*;

    fn git_apply(root: &Path, patch: &[u8], extra_args: &[&str]) {
        let mut git = Command::new("git")
            .current_dir(root)
            .args(["-c", "core.autocrlf=false", "-c", "apply.whitespace=nowarn", "apply"])
            .args(extra_args)
            .arg("-")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        git.stdin.take().unwrap().write_all(patch).unwrap();
        let result = git.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(patch),
        );
    }

    fn change(path: &str, before: &str, after: &str) -> Change {
        Change {
            path: PathBuf::from(path),
            before: before.to_owned(),
            after: after.to_owned(),
        }
    }

    #[test]
    fn git_applies_multiple_files_with_empty_ranges_and_original_line_endings() {
        let project = tempfile::tempdir().unwrap();
        let before = (1..=30)
            .map(|line| format!("line {line}\r\n"))
            .collect::<String>();
        let after = before
            .replace("line 1\r\n", "changed\r\n")
            .replace("line 30\r\n", "last");
        let changes = [
            change("dir with spaces/示例.txt", &before, &after),
            change("empty.txt", "", "added\n"),
            change("cleared.txt", "removed", ""),
            change("final.txt", "one\nno newline", "ONE\nno newline"),
            change("literal.txt", "original\n", "text\0content\n"),
        ];
        let mut patch = Vec::new();
        for change in &changes {
            let file = project.path().join(&change.path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, &change.before).unwrap();
            write_change(&mut patch, change).unwrap();
        }
        assert_eq!(String::from_utf8_lossy(&patch).matches("\n@@ -").count(), 6);
        git_apply(project.path(), &patch, &["--check"]);
        git_apply(project.path(), &patch, &[]);
        for change in &changes {
            assert_eq!(
                fs::read(project.path().join(&change.path)).unwrap(),
                change.after.as_bytes()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn git_applies_c_quoted_paths() {
        use std::os::unix::ffi::OsStrExt;

        assert_eq!(
            quoted_path('a', Path::new(std::ffi::OsStr::from_bytes(b"file\xff"))),
            "\"a/file\\377\""
        );
        let project = tempfile::tempdir().unwrap();
        let change = change(
            "quote\"tab\tline\nbackslash\\.txt",
            "original\n",
            "changed\n",
        );
        let file = project.path().join(&change.path);
        fs::write(&file, &change.before).unwrap();
        let mut patch = Vec::new();
        write_change(&mut patch, &change).unwrap();
        git_apply(project.path(), &patch, &["--check"]);
        git_apply(project.path(), &patch, &[]);
        assert_eq!(fs::read(file).unwrap(), change.after.as_bytes());
    }
}
