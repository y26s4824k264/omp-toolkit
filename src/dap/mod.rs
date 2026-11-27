//! Persistent multi-session DAP debugger facade.

pub mod client;
pub mod config;
pub mod session;

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    dap::{
        config::{list_adapters, resolve_adapter},
        session::DapManager,
    },
    path_guard::{resolve_file_in_root, resolve_in_root, resolve_root},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DebugInput {
    /// Action: `adapters`, `sessions`, `launch`, `attach`, `set_breakpoint`,
    /// `remove_breakpoint`, `continue`, `pause`, `step_over`, `step_in`, `step_out`,
    /// `threads`, `stack_trace`, `scopes`, `variables`, `evaluate`, `output`, `terminate`.
    pub action: String,
    /// Absolute project root directory (required for `adapters`, `launch`, `attach`, breakpoints).
    pub root: Option<PathBuf>,
    /// Optional debug session ID (auto-selected when only one session is active).
    pub session_id: Option<String>,
    /// Target executable, script, or Go package directory for `launch`.
    pub program: Option<PathBuf>,
    /// Program CLI arguments for `launch`.
    pub args: Option<Vec<String>>,
    /// Debug adapter ID (e.g. `lldb-dap`, `gdb`, `debugpy`, `dlv`, `codelldb`). Auto-inferred if omitted.
    pub adapter: Option<String>,
    /// Pause immediately at program entry on `launch` (default: true).
    pub stop_on_entry: Option<bool>,
    /// Process ID for `attach`.
    pub pid: Option<u32>,
    /// Host for remote TCP `attach` (default: 127.0.0.1).
    pub host: Option<String>,
    /// Port for remote TCP `attach`.
    pub port: Option<u16>,
    /// Source file path for `set_breakpoint` / `remove_breakpoint`.
    pub file: Option<PathBuf>,
    /// 1-based line number for breakpoints.
    pub line: Option<u32>,
    /// Optional breakpoint condition expression.
    pub condition: Option<String>,
    /// Optional breakpoint hit count condition.
    pub hit_condition: Option<String>,
    /// Max stack frames for `stack_trace` (default: 20).
    pub levels: Option<u32>,
    /// Stack frame ID for `scopes` or `evaluate`.
    pub frame_id: Option<i64>,
    /// Variable reference handle for `variables`.
    pub variable_ref: Option<i64>,
    /// Expression string for `evaluate`.
    pub expression: Option<String>,
    /// Evaluation context (`repl`, `watch`, `hover`).
    pub context: Option<String>,
    /// Clear buffered program output after reading (`output` action, default: false).
    pub clear: Option<bool>,
    /// Per-request timeout in seconds (5 to 120, default: 30).
    pub timeout_sec: Option<u64>,
}

/// Execute a DAP action against the persistent debug session manager.
pub async fn execute(manager: &DapManager, input: DebugInput) -> Result<Value> {
    let action = input.action.trim().to_ascii_lowercase();
    let timeout = Duration::from_secs(input.timeout_sec.unwrap_or(30).clamp(5, 120));

    match action.as_str() {
        "adapters" => {
            let root = match &input.root {
                Some(r) => resolve_root(r)?,
                None => std::env::current_dir()?,
            };
            let adapters = list_adapters(&root)?;
            Ok(json!({ "adapters": adapters }))
        }
        "sessions" => Ok(json!({ "sessions": manager.list_sessions().await })),
        "launch" => {
            let root = resolve_root(input.root.as_ref().context("launch requires `root`")?)?;
            let prog_in = input.program.as_ref().context("launch requires `program`")?;
            let program = if prog_in.is_absolute() {
                prog_in
                    .canonicalize()
                    .with_context(|| format!("resolve program {}", prog_in.display()))?
            } else {
                resolve_in_root(&root, prog_in)?
            };
            let adapter = resolve_adapter(&root, input.adapter.as_deref(), Some(&program))?;
            let session = manager
                .launch(
                    adapter,
                    root,
                    program,
                    input.args.unwrap_or_default(),
                    input.stop_on_entry.unwrap_or(true),
                    timeout,
                )
                .await?;
            Ok(json!({ "snapshot": session.snapshot().await }))
        }
        "attach" => {
            let root = resolve_root(input.root.as_ref().context("attach requires `root`")?)?;
            let adapter = resolve_adapter(&root, input.adapter.as_deref(), None)?;
            let session = manager
                .attach(adapter, root, input.pid, input.host, input.port, timeout)
                .await?;
            Ok(json!({ "snapshot": session.snapshot().await }))
        }
        "set_breakpoint" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            let file_in = input.file.as_ref().context("set_breakpoint requires `file`")?;
            let root = match &input.root {
                Some(r) => resolve_root(r)?,
                None => session.cwd.clone(),
            };
            let file = resolve_file_in_root(&root, file_in)?;
            let line = input.line.context("set_breakpoint requires `line`")?;
            session
                .set_breakpoint(file, line, input.condition, input.hit_condition, timeout)
                .await
        }
        "remove_breakpoint" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            let file_in = input
                .file
                .as_ref()
                .context("remove_breakpoint requires `file`")?;
            let root = match &input.root {
                Some(r) => resolve_root(r)?,
                None => session.cwd.clone(),
            };
            let file = resolve_file_in_root(&root, file_in)?;
            session.remove_breakpoint(file, input.line, timeout).await
        }
        "continue" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session.resume("continue", timeout).await
        }
        "step_over" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session.resume("next", timeout).await
        }
        "step_in" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session.resume("stepIn", timeout).await
        }
        "step_out" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session.resume("stepOut", timeout).await
        }
        "pause" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session.pause(timeout).await
        }
        "threads" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session.threads(timeout).await
        }
        "stack_trace" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            session
                .stack_trace(input.levels.unwrap_or(20).clamp(1, 200), timeout)
                .await
        }
        "scopes" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            let frame_id = input.frame_id.context("scopes requires `frame_id`")?;
            session.scopes(frame_id, timeout).await
        }
        "variables" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            let var_ref = input
                .variable_ref
                .context("variables requires `variable_ref`")?;
            session.variables(var_ref, timeout).await
        }
        "evaluate" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            let expr = input
                .expression
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .context("evaluate requires `expression`")?;
            session
                .evaluate(expr, input.frame_id, input.context.as_deref(), timeout)
                .await
        }
        "output" => {
            let session = manager.get_session(input.session_id.as_deref()).await?;
            Ok(session.drain_output(input.clear.unwrap_or(false)).await)
        }
        "terminate" => manager.terminate(input.session_id.as_deref()).await,
        other => bail!(
            "unknown debug action '{other}'; expected adapters, sessions, launch, attach, set_breakpoint, remove_breakpoint, continue, pause, step_over, step_in, step_out, threads, stack_trace, scopes, variables, evaluate, output, or terminate"
        ),
    }
}
