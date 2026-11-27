//! Lightweight lexical command structure analyzer for standalone minimizer use.
//!
//! Replaces the embedded bash 5.3 AST parser dependency from `omp-shell` with a
//! zero-dependency quote-aware shell command classifier covering the exact same
//! `CommandPlan` categories: `Single`, `Piped`, `Chain`, `Compound`, and `Unsupported`.

/// One segment of a safe `&&` / `;` chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSegment {
    pub command: String,
    pub source_at: u32,
    pub source_len: u32,
    pub program: String,
    pub run_if_previous_succeeded: bool,
    pub suppress_errexit: bool,
}

/// Outcome of analyzing a raw command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPlan {
    Single { program: String },
    Piped,
    Chain { segments: Vec<ChainSegment> },
    Compound,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SepKind {
    And,
    Semi,
}

/// Classify a shell command string without requiring a full bash runtime.
#[must_use]
pub fn analyze(command: &str) -> CommandPlan {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandPlan::Unsupported;
    }

    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut seg_start = 0usize;
    let mut parts: Vec<(usize, usize, Option<SepKind>)> = Vec::new();
    let mut saw_pipe = false;
    let mut saw_compound = false;

    while i < len {
        let b = bytes[i];
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if b == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_double = false;
            } else if b == b'`' || (b == b'$' && i + 1 < len && bytes[i + 1] == b'(') {
                saw_compound = true;
            }
            i += 1;
            continue;
        }

        match b {
            b'\\' => {
                if i + 1 >= len {
                    return CommandPlan::Unsupported;
                }
                i += 2;
            }
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'#' if i == 0 || bytes[i - 1].is_ascii_whitespace() => {
                // Rest of line is a shell comment.
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'`' => {
                saw_compound = true;
                i += 1;
            }
            b'$' if i + 1 < len && bytes[i + 1] == b'(' => {
                saw_compound = true;
                i += 2;
            }
            b'(' | b')' => {
                saw_compound = true;
                i += 1;
            }
            b'<' if i + 1 < len && (bytes[i + 1] == b'<' || bytes[i + 1] == b'(') => {
                saw_compound = true;
                i += 2;
            }
            b'>' if i + 1 < len && bytes[i + 1] == b'(' => {
                saw_compound = true;
                i += 2;
            }
            b'|' => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    saw_compound = true;
                    i += 2;
                } else {
                    saw_pipe = true;
                    i += 1;
                }
            }
            b'&' => {
                if i + 1 < len && bytes[i + 1] == b'&' {
                    parts.push((seg_start, i, Some(SepKind::And)));
                    i += 2;
                    seg_start = i;
                } else if i > 0 && matches!(bytes[i - 1], b'>' | b'<') {
                    // fd duplication such as 2>&1
                    i += 1;
                } else if i + 1 < len && bytes[i + 1] == b'>' {
                    // &> redirect
                    i += 2;
                } else {
                    saw_compound = true;
                    i += 1;
                }
            }
            b';' | b'\n' => {
                if b == b';' && i + 1 < len && bytes[i + 1] == b';' {
                    saw_compound = true;
                    i += 2;
                } else {
                    parts.push((seg_start, i, Some(SepKind::Semi)));
                    i += 1;
                    seg_start = i;
                }
            }
            _ => i += 1,
        }
    }

    if in_single || in_double {
        return CommandPlan::Unsupported;
    }
    if saw_pipe {
        return CommandPlan::Piped;
    }
    if saw_compound {
        return CommandPlan::Compound;
    }

    parts.push((seg_start, len, None));

    let mut segments = Vec::new();
    let mut prev_and = false;
    for idx in 0..parts.len() {
        let (start, end, sep) = parts[idx];
        let raw_slice = &command[start..end];
        let trimmed_start = raw_slice.len() - raw_slice.trim_start().len();
        let slice = raw_slice.trim();
        if slice.is_empty() {
            if matches!(sep, Some(SepKind::And)) || prev_and {
                return CommandPlan::Unsupported;
            }
            continue;
        }
        let Some(program) = leading_program(slice) else {
            return CommandPlan::Unsupported;
        };
        if is_compound_keyword(&program) {
            return CommandPlan::Compound;
        }
        let abs_start = start + trimmed_start;
        let followed_by_and = matches!(sep, Some(SepKind::And));
        segments.push(ChainSegment {
            command: slice.to_owned(),
            source_at: u32::try_from(abs_start).unwrap_or(u32::MAX),
            source_len: u32::try_from(slice.len()).unwrap_or(u32::MAX),
            program,
            run_if_previous_succeeded: prev_and,
            suppress_errexit: followed_by_and,
        });
        prev_and = followed_by_and;
    }

    match segments.len() {
        0 => CommandPlan::Unsupported,
        1 => CommandPlan::Single {
            program: segments.remove(0).program,
        },
        _ => CommandPlan::Chain { segments },
    }
}

fn is_compound_keyword(word: &str) -> bool {
    matches!(
        word,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "select"
            | "function"
            | "{"
            | "}"
            | "[["
            | "coproc"
    )
}

fn leading_program(segment: &str) -> Option<String> {
    let words = split_shell_words(segment);
    let mut iter = words.into_iter().peekable();
    while let Some(word) = iter.next() {
        if word.starts_with('#') {
            break;
        }
        // Skip leading redirections like `>out` or `2>/dev/null`
        if is_redirection_only(&word) {
            if word.ends_with('>') || word.ends_with('<') {
                let _ = iter.next();
            }
            continue;
        }
        // Skip leading ENV=value assignments
        if is_env_assign(&word) {
            continue;
        }
        if word.is_empty() {
            return None;
        }
        return Some(word);
    }
    None
}

fn split_shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if in_double {
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if ch == '"' {
                in_double = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' => in_single = true,
            '"' => in_double = true,
            c if c.is_ascii_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn is_env_assign(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_redirection_only(word: &str) -> bool {
    let trimmed = word.trim_start_matches(|c: char| c.is_ascii_digit());
    trimmed.starts_with('>') || trimmed.starts_with('<')
}
