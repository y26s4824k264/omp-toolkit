//! Command-aware CLI output minimizer built directly on `omp-minimizer`
//! (extracted from Oh My Pi's `omp-shell` without bundling the 38k-line bash engine).

use std::path::PathBuf;

use anyhow::{Result, ensure};
use omp_minimizer::{MinimizerConfig, apply_with_min_chars};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::path_guard::{read_utf8_file, resolve_file_in_root, resolve_root};

const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MinimizeInput {
    /// Original command line used to select the right filter (e.g. `cargo test`, `git diff`, `pytest`).
    pub command: String,
    /// Command exit code (default: 0). Non-zero exit codes are still filtered to surface failures cleanly.
    pub exit_code: Option<i32>,
    /// Raw command output text. Provide either `output` or (`root` + `file`).
    pub output: Option<String>,
    /// Optional absolute project root when reading a saved log file.
    pub root: Option<PathBuf>,
    /// Optional project-relative log file path when reading from disk.
    pub file: Option<PathBuf>,
    /// Minimum character length before applying filters (default: 0 for explicit tool calls).
    pub min_chars: Option<usize>,
}

/// Minimize noisy build, test, VCS, or linter output while preserving errors and key summaries.
pub fn minimize_output(input: MinimizeInput) -> Result<Value> {
    ensure!(
        !input.command.trim().is_empty(),
        "command must not be empty"
    );
    let raw = match (input.output, input.root, input.file) {
        (Some(text), None, None) => text,
        (None, Some(root), Some(file)) => {
            let root = resolve_root(&root)?;
            let abs = resolve_file_in_root(&root, &file)?;
            read_utf8_file(&abs, MAX_LOG_BYTES)?
        }
        _ => anyhow::bail!("provide either `output` text or both `root` and `file`"),
    };

    let config = MinimizerConfig {
        enabled: true,
        ..MinimizerConfig::default()
    };
    let exit_code = input.exit_code.unwrap_or(0);
    let min_chars = input.min_chars.unwrap_or(0);
    let result = apply_with_min_chars(&input.command, &raw, exit_code, &config, min_chars);
    let saved_bytes = result.bytes_saved();
    let saved_percent = if result.input_bytes > 0 {
        (saved_bytes as f64 / result.input_bytes as f64) * 100.0
    } else {
        0.0
    };

    Ok(json!({
        "filter": result.filter,
        "changed": result.changed,
        "exit_code": exit_code,
        "input_bytes": result.input_bytes,
        "output_bytes": result.output_bytes,
        "saved_bytes": saved_bytes,
        "saved_percent": (saved_percent * 10.0).round() / 10.0,
        "text": result.text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimizes_failing_cargo_test_output() -> Result<()> {
        let mut raw = String::from("   Compiling demo v0.1.0\n    Finished `test` profile\n     Running unittests src/lib.rs\n\nrunning 30 tests\n");
        for i in 0..29 {
            raw.push_str(&format!("test suite::case_{i} ... ok\n"));
        }
        raw.push_str("test suite::broken_case ... FAILED\n\nfailures:\n\n---- suite::broken_case stdout ----\nassertion failed: 1 == 2\n\nfailures:\n    suite::broken_case\n\ntest result: FAILED. 29 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n");

        let res = minimize_output(MinimizeInput {
            command: "cargo test".into(),
            exit_code: Some(101),
            output: Some(raw),
            root: None,
            file: None,
            min_chars: Some(0),
        })?;
        assert_eq!(res["changed"], true);
        let text = res["text"].as_str().unwrap();
        assert!(text.contains("broken_case"), "{text}");
        assert!(!text.contains("case_15 ... ok"), "{text}");
        Ok(())
    }
}
