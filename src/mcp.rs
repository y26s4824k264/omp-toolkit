//! In-process stdio MCP server exposing 8 high-signal, non-intrusive OMP tools:
//! `code_summary`, `code_outline`, `ast_grep`, `ast_edit`, `lsp`, `debug`,
//! `minimize_output`, and `git_conflicts`.

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::Value;

use crate::{
    code_ast::{self, AstEditInput, AstGrepInput, OutlineInput, SummaryInput},
    conflicts::{self, ConflictsInput},
    dap::{self, DebugInput, session::DapManager},
    lsp::{self, LspInput, pool::LspPool},
    minimize::{self, MinimizeInput},
};

#[derive(Clone)]
pub struct OmpMcpServer {
    lsp_pool: Arc<LspPool>,
    dap_manager: Arc<DapManager>,
    tool_router: ToolRouter<Self>,
}

impl Default for OmpMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl OmpMcpServer {
    pub fn new() -> Self {
        Self {
            lsp_pool: Arc::new(LspPool::new()),
            dap_manager: Arc::new(DapManager::new()),
            tool_router: Self::tool_router(),
        }
    }

    pub async fn shutdown(&self) {
        self.lsp_pool.shutdown_all().await;
        self.dap_manager.shutdown_all().await;
    }
}

#[tool_router]
impl OmpMcpServer {
    #[tool(
        description = "Read a large source file as a tree-sitter AST-folded structural summary with line numbers and progressive BFS unfolding (`unfold_until_lines`), OR pass `block_line` to expand a single full syntactic block (including attributes/decorators) with exact line numbers.",
        annotations(read_only_hint = true)
    )]
    async fn code_summary(&self, Parameters(input): Parameters<SummaryInput>) -> CallToolResult {
        let res = tokio::task::spawn_blocking(move || code_ast::code_summary(input))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|inner| inner);
        text_result(res)
    }

    #[tool(
        description = "List top-level symbols (functions, structs, classes, interfaces, methods) and their line ranges in a source file using tree-sitter — instant, zero LSP startup overhead.",
        annotations(read_only_hint = true)
    )]
    async fn code_outline(&self, Parameters(input): Parameters<OutlineInput>) -> CallToolResult {
        let res = tokio::task::spawn_blocking(move || code_ast::code_outline(input))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|inner| inner);
        json_result(res)
    }

    #[tool(
        description = "Full-repository parallel AST structural search via tree-sitter & ast-grep. Supports directory/glob scoping, automatic language detection, multiple patterns, metavariable captures, and pagination.",
        annotations(read_only_hint = true)
    )]
    async fn ast_grep(&self, Parameters(input): Parameters<AstGrepInput>) -> CallToolResult {
        json_result(code_ast::ast_grep(input).await)
    }

    #[tool(
        description = "Full-repository parallel AST structural rewrite with post-rewrite tree-sitter syntax validation. Defaults to `apply: false` (returns unified diff preview without modifying disk); set `apply: true` to write atomically."
    )]
    async fn ast_edit(&self, Parameters(input): Parameters<AstEditInput>) -> CallToolResult {
        json_result(code_ast::ast_edit(input).await)
    }

    #[tool(
        description = "Persistent workspace LSP code intelligence (reuses warm language servers and auto-syncs external disk edits). Actions: `status`, `diagnostics`, `definition`, `references`, `hover`, `type_definition`, `implementation`, `symbols`, `workspace_symbols`, `code_actions`, `rename` (diff preview / `apply: true`), `rename_file` (updates imports via willRenameFiles), `shutdown`."
    )]
    async fn lsp(&self, Parameters(input): Parameters<LspInput>) -> CallToolResult {
        json_result(lsp::execute(&self.lsp_pool, input).await)
    }

    #[tool(
        description = "Persistent multi-session DAP debugger supporting 14 adapters (lldb-dap, gdb, codelldb, debugpy, dlv, js-debug-adapter, etc.). Actions: `adapters`, `sessions`, `launch`, `attach`, `set_breakpoint`, `remove_breakpoint`, `continue`, `pause`, `step_over`, `step_in`, `step_out`, `threads`, `stack_trace`, `scopes`, `variables`, `evaluate`, `output`, `terminate`."
    )]
    async fn debug(&self, Parameters(input): Parameters<DebugInput>) -> CallToolResult {
        json_result(dap::execute(&self.dap_manager, input).await)
    }

    #[tool(
        description = "Compress verbose CLI output (cargo, git, pytest, jest, tsc, go, docker, gradle, etc.) using OMP's command-aware filters while preserving failure details and summaries.",
        annotations(read_only_hint = true)
    )]
    async fn minimize_output(&self, Parameters(input): Parameters<MinimizeInput>) -> CallToolResult {
        let res = tokio::task::spawn_blocking(move || minimize::minimize_output(input))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|inner| inner);
        json_result(res)
    }

    #[tool(
        description = "Scan a file or the entire repository for unresolved Git merge conflict markers (2-way and diff3 3-way) and resolve them structurally (`ours`, `theirs`, `base`, `both`, `both_reverse`, `delete`, or `custom` with `@ours`/`@theirs`/`@base`/`@both` tokens). Defaults to `apply: false` (unified diff preview); set `apply: true` to write atomically."
    )]
    async fn git_conflicts(&self, Parameters(input): Parameters<ConflictsInput>) -> CallToolResult {
        json_result(conflicts::execute(input).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OmpMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("omp-toolkit", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Native Oh My Pi code intelligence server: use `code_summary` (with optional `block_line` expansion) and `code_outline` for low-token code reading, `ast_grep`/`ast_edit` for repo-wide structural search & refactors (dry-run by default), `lsp` for persistent IDE diagnostics/navigation/rename, `debug` for live DAP debugging, `minimize_output` to compact noisy command logs, and `git_conflicts` to scan and resolve Git merge conflicts cleanly.",
            )
    }
}

/// Serve the MCP server over stdio until EOF, then shut down all warm LSP/DAP child processes.
pub async fn serve_stdio() -> Result<()> {
    let server = OmpMcpServer::new();
    let cleanup = server.clone();
    let running = server.serve(stdio()).await?;
    let wait_res = running.waiting().await;
    cleanup.shutdown().await;
    wait_res?;
    Ok(())
}

fn text_result(result: Result<String>) -> CallToolResult {
    match result {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(err) => CallToolResult::error(vec![ContentBlock::text(format!("{err:#}"))]),
    }
}

fn json_result(result: Result<Value>) -> CallToolResult {
    match result {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(err) => CallToolResult::error(vec![ContentBlock::text(format!("{err:#}"))]),
        },
        Err(err) => CallToolResult::error(vec![ContentBlock::text(format!("{err:#}"))]),
    }
}
