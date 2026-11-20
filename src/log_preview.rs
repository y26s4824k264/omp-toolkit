use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use anyhow::Result;

const MAX_LINE_BYTES: usize = 2_048;
const LINE_GAP: &str = "…";

/// Read bounded head and tail windows from the completed command output.
pub(crate) fn log_excerpt(mut log: File, limit: usize) -> Result<String> {
    let total = log.metadata()?.len();
    if total <= limit as u64 {
        let (mut text, kept) = render_lines(&read_window(&mut log, total as usize)?);
        if kept as u64 == total {
            return Ok(text);
        }
        let notice = omission(total - kept as u64);
        if text.len() + notice.len() <= limit {
            text.push_str(&notice);
            return Ok(text);
        }
        log.seek(SeekFrom::Start(0))?;
    }

    let largest_notice = omission(total);
    if largest_notice.len() >= limit {
        return Ok(largest_notice[..limit].to_owned());
    }
    let available = limit - largest_notice.len();
    let head_limit = available / 2;
    let tail_limit = available - head_limit;

    let head = read_window(&mut log, head_limit)?;
    log.seek(SeekFrom::Start(total.saturating_sub(tail_limit as u64)))?;
    let tail = read_window(&mut log, tail_limit)?;

    // Prefer complete lines. A window entirely inside a long line still
    // contributes a bounded fragment, including a last line without a newline.
    let head = match head.iter().rposition(|&byte| byte == b'\n') {
        Some(end) => &head[..=end],
        None => complete_utf8_end(&head),
    };
    let tail = match tail.iter().position(|&byte| byte == b'\n') {
        Some(end) if end + 1 < tail.len() => &tail[end + 1..],
        _ => complete_utf8_start(&tail),
    };

    let (head, head_kept) = render_lines(head);
    let (tail, tail_kept) = render_lines(tail);
    let omitted = total - (head_kept + tail_kept) as u64;
    Ok(format!("{head}{}{tail}", omission(omitted)))
}

fn read_window(log: &mut impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    log.take(limit as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn omission(bytes: u64) -> String {
    format!("\n[{bytes} log bytes omitted]\n")
}

fn render_lines(bytes: &[u8]) -> (String, usize) {
    let mut text = String::with_capacity(bytes.len());
    let mut kept = 0;
    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        if line.len() <= MAX_LINE_BYTES {
            append_utf8(&mut text, line);
            kept += line.len();
            continue;
        }
        let body = line
            .strip_suffix(b"\r\n")
            .or_else(|| line.strip_suffix(b"\n"))
            .unwrap_or(line);
        let ending = &line[body.len()..];
        let available = MAX_LINE_BYTES - LINE_GAP.len() - ending.len();
        let prefix = complete_utf8_end(&body[..available / 2]);
        let suffix = complete_utf8_start(&body[body.len() - (available - available / 2)..]);
        append_utf8(&mut text, prefix);
        text.push_str(LINE_GAP);
        append_utf8(&mut text, suffix);
        append_utf8(&mut text, ending);
        kept += prefix.len() + suffix.len() + ending.len();
    }
    (text, kept)
}

fn complete_utf8_end(bytes: &[u8]) -> &[u8] {
    let Some(chunk) = bytes.utf8_chunks().last() else {
        return bytes;
    };
    let invalid = chunk.invalid();
    if matches!(std::str::from_utf8(invalid), Err(error) if error.error_len().is_none()) {
        &bytes[..bytes.len() - invalid.len()]
    } else {
        bytes
    }
}

fn complete_utf8_start(bytes: &[u8]) -> &[u8] {
    let continuation_bytes = bytes
        .iter()
        .take_while(|&&byte| byte & 0b1100_0000 == 0b1000_0000)
        .count();
    &bytes[continuation_bytes..]
}

fn append_utf8(text: &mut String, bytes: &[u8]) {
    // A one-byte replacement for malformed data keeps the UTF-8 preview
    // within the byte allowance even when the log contains many bad bytes.
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            text.push('?');
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn excerpt(bytes: &[u8], limit: usize) -> String {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        log_excerpt(file.reopen().unwrap(), limit).unwrap()
    }

    #[test]
    fn keeps_short_logs_and_the_end_of_long_logs() {
        let short = "build started\r\nbuild finished\n";
        assert_eq!(excerpt(short.as_bytes(), 12_000), short);

        let log = format!(
            "build started\n{}ERROR: final stage failed",
            "working on middleware\n".repeat(800)
        );
        let preview = excerpt(log.as_bytes(), 12_000);
        assert!(preview.starts_with("build started\n"));
        assert!(preview.ends_with("ERROR: final stage failed"));
        assert!(preview.contains("log bytes omitted"));
        assert!(preview.len() <= 12_000);
    }

    #[test]
    fn limits_huge_lines_without_breaking_utf8() {
        let log = format!("{}{}{}", "前".repeat(600), "後".repeat(600), "tail");
        let preview = excerpt(log.as_bytes(), 12_000);
        assert!(preview.contains("…"));
        assert!(preview.contains("tail"));
        assert!(preview.lines().all(|line| line.len() <= MAX_LINE_BYTES));

        let mut malformed = vec![0xff; 12_500];
        malformed.extend_from_slice("最後のエラー\n".as_bytes());
        let preview = excerpt(&malformed, 12_000);
        assert!(preview.ends_with("最後のエラー\n"));
        assert!(preview.len() <= 12_000);
    }
}
