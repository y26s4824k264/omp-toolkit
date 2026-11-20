use std::{fs, path::Path};

use anyhow::{Context, Result};
use omp_offline_claude::{Family, count_units};
use serde_json::json;
use tiktoken_rs::{cl100k_base_singleton, o200k_base_singleton};

pub(crate) fn count_file(path: &Path) -> Result<String> {
    let contents = fs::read(path).context("read the token count input")?;
    let decoded = String::from_utf8_lossy(&contents);
    let text = decoded.strip_prefix('\u{feff}').unwrap_or(&decoded);
    let utf16: Vec<_> = text.encode_utf16().collect();
    let report = json!({
        "source": { "kind": "file", "path": path.to_string_lossy() },
        "bytes": text.len(),
        "chars": text.chars().count(),
        "lines": text.lines().count(),
        "encodings": [
            {
                "encoding": "O200kBase",
                "name": "o200k_base",
                "models": "GPT-4o · o1 · GPT-5+",
                "tokens": o200k_base_singleton().count_ordinary(text),
            },
            {
                "encoding": "Cl100kBase",
                "name": "cl100k_base",
                "models": "GPT-3.5 · GPT-4",
                "tokens": cl100k_base_singleton().count_ordinary(text),
            },
            {
                "encoding": "ClaudeV3",
                "name": "claude-v3",
                "models": "Claude 3 … Opus 4.6",
                "tokens": count_units(&utf16, Family::V3),
            },
            {
                "encoding": "ClaudeV47",
                "name": "claude-v47",
                "models": "Opus 4.7–4.9",
                "tokens": count_units(&utf16, Family::V47),
            },
            {
                "encoding": "ClaudeV5",
                "name": "claude-v5",
                "models": "Opus 5+",
                "tokens": count_units(&utf16, Family::V5),
            },
            {
                "encoding": "ClaudeV5Sonnet",
                "name": "claude-v5-sonnet",
                "models": "Sonnet/Fable 5+ · Haiku 5.5",
                "tokens": count_units(&utf16, Family::V5Sonnet),
            },
        ],
    });
    serde_json::to_string_pretty(&report).context("serialize the token count")
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::Value;
    use tempfile::NamedTempFile;

    use super::count_file;

    #[test]
    fn counts_unicode_scalars_and_file_lines() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("A\n你好🙂\n".as_bytes()).unwrap();

        let count: Value = serde_json::from_str(&count_file(file.path()).unwrap()).unwrap();
        assert_eq!(count["source"]["kind"], "file");
        assert_eq!(count["bytes"], 13);
        assert_eq!(count["chars"], 6);
        assert_eq!(count["lines"], 2);
        let encodings: Vec<_> = count["encodings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap(),
                    row["tokens"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            encodings,
            [
                ("o200k_base", 5),
                ("cl100k_base", 7),
                ("claude-v3", 7),
                ("claude-v47", 7),
                ("claude-v5", 7),
                ("claude-v5-sonnet", 7),
            ]
        );
    }

    #[test]
    fn empty_file_matches_upstream_token_counts() {
        let file = NamedTempFile::new().unwrap();
        let count: Value = serde_json::from_str(&count_file(file.path()).unwrap()).unwrap();
        assert_eq!(count["bytes"], 0);
        assert_eq!(count["chars"], 0);
        assert_eq!(count["lines"], 0);
        assert_eq!(token_values(&count), [0, 0, 1, 1, 0, 0]);
    }

    #[test]
    fn bom_and_trailing_space_match_upstream_counts() {
        for (text, bytes, expected) in [
            ("\u{feff}x\n", 2, [2, 2, 1, 1, 1, 1]),
            ("a\n ", 3, [3, 3, 3, 3, 1, 3]),
        ] {
            let mut file = NamedTempFile::new().unwrap();
            file.write_all(text.as_bytes()).unwrap();
            let count: Value = serde_json::from_str(&count_file(file.path()).unwrap()).unwrap();
            assert_eq!(count["bytes"], bytes);
            assert_eq!(token_values(&count), expected);
        }
    }

    fn token_values(count: &Value) -> Vec<u64> {
        count["encodings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|encoding| encoding["tokens"].as_u64().unwrap())
            .collect()
    }
}
