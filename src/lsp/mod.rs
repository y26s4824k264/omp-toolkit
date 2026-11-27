//! Persistent LSP facade exposing IDE-grade code intelligence over MCP and CLI.

pub mod client;
pub mod config;
pub mod edit;
pub mod install;
pub mod pool;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    lsp::{
        client::LspClient,
        config::{
            ResolvedServer, ServerDef, is_vue_file, load_definitions, resolve_vue_ts_companion,
            select_servers, server_status,
        },
        edit::{FileMove, apply_or_preview_workspace_edits, collect_workspace_edits},
        pool::LspPool,
    },
    path_guard::{
        display_relative, file_uri_to_path, path_to_file_uri, resolve_file_in_root,
        resolve_lsp_position, resolve_new_path_in_root, resolve_root,
    },
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LspInput {
    /// Action to perform: `status`, `diagnostics`, `definition`, `references`, `hover`,
    /// `type_definition`, `implementation`, `symbols`, `workspace_symbols`, `code_actions`,
    /// `rename`, `rename_file`, or `shutdown`.
    pub action: String,
    /// Absolute project root directory.
    pub root: PathBuf,
    /// Project-relative target file (required for file-scoped actions).
    pub file: Option<PathBuf>,
    /// Additional files for batch `diagnostics`.
    pub files: Option<Vec<PathBuf>>,
    /// 1-based line number (optional when `symbol` is supplied).
    pub line: Option<u32>,
    /// 1-based column number (optional when `symbol` is supplied).
    pub column: Option<u32>,
    /// Symbol name (or `name#occurrence`) to locate position automatically without counting columns.
    pub symbol: Option<String>,
    /// Search query for `workspace_symbols`.
    pub query: Option<String>,
    /// New symbol name (for `rename`) or new relative path/filename (for `rename_file`).
    pub new_name: Option<String>,
    /// For `rename` / `rename_file`: false (default) previews unified diff; true applies atomically.
    pub apply: Option<bool>,
    /// Per-request timeout in seconds (5 to 120, default 25).
    pub timeout_sec: Option<u64>,
}

/// Execute an LSP action against the persistent connection pool.
pub async fn execute(pool: &LspPool, input: LspInput) -> Result<Value> {
    let action = input.action.trim().to_ascii_lowercase();
    if action == "shutdown" {
        pool.shutdown_all().await;
        return Ok(json!({ "status": "shutdown_complete" }));
    }

    let root = resolve_root(&input.root)?;
    let timeout = Duration::from_secs(input.timeout_sec.unwrap_or(25).clamp(5, 120));
    let defs = load_definitions(&root)?;

    match action.as_str() {
        "status" => {
            let active: Vec<Value> = pool
                .active_servers()
                .await
                .into_iter()
                .map(|(name, ws)| json!({ "name": name, "workspace_root": ws }))
                .collect();
            let servers = server_status(&defs, &root);
            let available_matching: Vec<&Value> = servers
                .iter()
                .filter(|s| s["available"] == true && s["matches_root"] == true)
                .collect();
            let installed_servers: Vec<&str> = servers
                .iter()
                .filter(|s| s["available"] == true)
                .filter_map(|s| s["name"].as_str())
                .collect();
            Ok(json!({
                "root": root,
                "active_connections": active,
                "ready_for_project": available_matching,
                "installed_servers": installed_servers,
                "managed_lsp_dir": install::managed_lsp_root().map(|p| p.display().to_string()),
                "total_catalog_servers": servers.len(),
                "hint": "Run `omp-toolkit lsp-install` to auto-provision missing language servers (TypeScript, Vue, HTML/CSS/JSON/ESLint, Python, YAML, Bash, Docker, Tailwind, Svelte, Astro, GraphQL, Prisma, PHP, Go, Rust, Ruff).",
            }))
        }
        "diagnostics" => {
            let mut targets = Vec::new();
            if let Some(f) = input.file {
                targets.push(resolve_file_in_root(&root, &f)?);
            }
            if let Some(list) = input.files {
                for f in list {
                    let abs = resolve_file_in_root(&root, &f)?;
                    if !targets.contains(&abs) {
                        targets.push(abs);
                    }
                }
            }
            ensure!(
                !targets.is_empty(),
                "diagnostics requires `file` or `files`"
            );
            let mut reports = Vec::with_capacity(targets.len());
            for abs in targets {
                let display = display_relative(&root, &abs);
                let candidates = select_servers(&defs, &root, Some(&abs), "diagnostics");
                if candidates.is_empty() {
                    reports.push(json!({
                        "file": display,
                        "status": "no_server",
                        "diagnostics": [],
                    }));
                    continue;
                }
                let mut selected = Vec::new();
                if is_vue_file(&abs) {
                    selected.extend(candidates.iter().filter(|s| !s.def.is_linter));
                } else if let Some(p) = candidates.iter().find(|s| !s.def.is_linter) {
                    selected.push(p);
                }
                selected.extend(candidates.iter().filter(|s| s.def.is_linter));

                let mut all_diags = Vec::new();
                let mut servers_used = Vec::new();
                let mut errors = Vec::new();
                for srv in selected {
                    match get_or_start_server(pool, &defs, &root, srv, timeout).await {
                        Ok(client) => {
                            let _ = client.reconcile_open_files().await;
                            match client.diagnostics(&abs, timeout).await {
                                Ok(items) => {
                                    servers_used.push(srv.def.name.clone());
                                    for item in items {
                                        all_diags.push(format_diagnostic(&srv.def.name, &item));
                                    }
                                }
                                Err(e) => errors.push(format!("{}: {e:#}", srv.def.name)),
                            }
                        }
                        Err(e) => errors.push(format!("{}: {e:#}", srv.def.name)),
                    }
                }
                reports.push(json!({
                    "file": display,
                    "servers": servers_used,
                    "error_count": all_diags.iter().filter(|d| d["severity"] == "error").count(),
                    "diagnostics": all_diags,
                    "server_errors": errors,
                }));
            }
            Ok(json!({ "reports": reports }))
        }
        "workspace_symbols" => {
            let query = input
                .query
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .context("workspace_symbols requires `query`")?;
            let candidates = select_servers(&defs, &root, None, "workspace_symbols");
            ensure!(
                !candidates.is_empty(),
                "no available language server matches this workspace root"
            );
            let mut results = Vec::new();
            let mut servers = Vec::new();
            let mut failures = Vec::new();
            for srv in candidates {
                match pool.get_or_start(&srv, timeout).await {
                    Ok(client) => {
                        if !client.supports("workspaceSymbolProvider", "workspace/symbol").await {
                            continue;
                        }
                        client.wait_for_index(timeout.min(Duration::from_secs(10))).await;
                        match client
                            .request("workspace/symbol", json!({ "query": query }), timeout)
                            .await
                        {
                            Ok(Value::Array(items)) => {
                                servers.push(srv.def.name.clone());
                                for item in items.into_iter().take(100) {
                                    results.push(simplify_location_item(&root, item));
                                }
                            }
                            Ok(_) => servers.push(srv.def.name.clone()),
                            Err(e) => failures.push(format!("{}: {e:#}", srv.def.name)),
                        }
                    }
                    Err(e) => failures.push(format!("{}: {e:#}", srv.def.name)),
                }
            }
            Ok(json!({
                "query": query,
                "servers": servers,
                "count": results.len(),
                "symbols": results,
                "failures": failures,
            }))
        }
        "definition" | "references" | "hover" | "type_definition" | "implementation"
        | "symbols" | "code_actions" => {
            let file = input
                .file
                .as_ref()
                .with_context(|| format!("{action} requires `file`"))?;
            let abs = resolve_file_in_root(&root, file)?;
            let display = display_relative(&root, &abs);
            let candidates = select_servers(&defs, &root, Some(&abs), &action);
            ensure!(
                !candidates.is_empty(),
                "no installed language server handles {display}"
            );
            let (cap_key, method) = match action.as_str() {
                "definition" => ("definitionProvider", "textDocument/definition"),
                "references" => ("referencesProvider", "textDocument/references"),
                "hover" => ("hoverProvider", "textDocument/hover"),
                "type_definition" => ("typeDefinitionProvider", "textDocument/typeDefinition"),
                "implementation" => ("implementationProvider", "textDocument/implementation"),
                "symbols" => ("documentSymbolProvider", "textDocument/documentSymbol"),
                "code_actions" => ("codeActionProvider", "textDocument/codeAction"),
                _ => unreachable!(),
            };

            let mut failures = Vec::new();
            let mut empty_fallback: Option<Value> = None;
            for srv in candidates {
                let client = match get_or_start_server(pool, &defs, &root, &srv, timeout).await {
                    Ok(c) => c,
                    Err(e) => {
                        failures.push(format!("{}: {e:#}", srv.def.name));
                        continue;
                    }
                };
                if !client.supports(cap_key, method).await {
                    continue;
                }
                let _ = client.reconcile_open_files().await;
                let doc = client.sync_file(&abs).await?;
                if matches!(action.as_str(), "references" | "definition" | "implementation") {
                    client.wait_for_index(timeout.min(Duration::from_secs(10))).await;
                }
                let params = match action.as_str() {
                    "symbols" => json!({ "textDocument": { "uri": doc.uri } }),
                    "code_actions" => {
                        let (l, c) = resolve_lsp_position(
                            &doc.text,
                            input.line,
                            input.column,
                            input.symbol.as_deref(),
                        )?;
                        json!({
                            "textDocument": { "uri": doc.uri },
                            "range": {
                                "start": { "line": l, "character": c },
                                "end": { "line": l, "character": c },
                            },
                            "context": { "diagnostics": [] },
                        })
                    }
                    _ => {
                        let (l, c) = resolve_lsp_position(
                            &doc.text,
                            input.line,
                            input.column,
                            input.symbol.as_deref(),
                        )?;
                        let mut p = json!({
                            "textDocument": { "uri": doc.uri },
                            "position": { "line": l, "character": c },
                        });
                        if action == "references" {
                            p["context"] = json!({ "includeDeclaration": true });
                        }
                        p
                    }
                };

                match client.request(method, params, timeout).await {
                    Ok(val) => {
                        let formatted = match action.as_str() {
                            "definition" | "references" | "type_definition" | "implementation" => {
                                simplify_locations(&root, val)
                            }
                            _ => val,
                        };
                        let payload = json!({
                            "file": display,
                            "server": srv.def.name,
                            "result": formatted,
                        });
                        if !is_empty_lsp_result(&payload["result"]) {
                            return Ok(payload);
                        }
                        if empty_fallback.is_none() {
                            empty_fallback = Some(payload);
                        }
                    }
                    Err(e) => failures.push(format!("{}: {e:#}", srv.def.name)),
                }
            }
            if let Some(fallback) = empty_fallback {
                return Ok(fallback);
            }
            bail!(
                "no language server completed `{action}` for {display}: {}",
                failures.join("; ")
            );
        }
        "rename" => {
            let file = input.file.as_ref().context("rename requires `file`")?;
            let new_name = input
                .new_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("rename requires `new_name`")?;
            let abs = resolve_file_in_root(&root, file)?;
            let display = display_relative(&root, &abs);
            let candidates = select_servers(&defs, &root, Some(&abs), "rename");
            ensure!(
                !candidates.is_empty(),
                "no installed language server handles {display}"
            );
            let mut failures = Vec::new();
            for srv in candidates {
                let client = match get_or_start_server(pool, &defs, &root, &srv, timeout).await {
                    Ok(c) => c,
                    Err(e) => {
                        failures.push(format!("{}: {e:#}", srv.def.name));
                        continue;
                    }
                };
                if !client.supports("renameProvider", "textDocument/rename").await {
                    continue;
                }
                let _ = client.reconcile_open_files().await;
                let doc = client.sync_file(&abs).await?;
                let (l, c) = resolve_lsp_position(
                    &doc.text,
                    input.line,
                    input.column,
                    input.symbol.as_deref(),
                )?;
                client.wait_for_index(timeout.min(Duration::from_secs(10))).await;
                match client
                    .request(
                        "textDocument/rename",
                        json!({
                            "textDocument": { "uri": doc.uri },
                            "position": { "line": l, "character": c },
                            "newName": new_name,
                        }),
                        timeout,
                    )
                    .await
                {
                    Ok(edit_val) => {
                        let by_uri = collect_workspace_edits(&[edit_val])?;
                        let apply = input.apply.unwrap_or(false);
                        let mut res = apply_or_preview_workspace_edits(&root, by_uri, None, apply)?;
                        res["server"] = json!(srv.def.name);
                        if apply {
                            let _ = client.reconcile_open_files().await;
                        }
                        return Ok(res);
                    }
                    Err(e) => failures.push(format!("{}: {e:#}", srv.def.name)),
                }
            }
            bail!(
                "no language server supports rename for {display}: {}",
                failures.join("; ")
            );
        }
        "rename_file" => {
            let file = input.file.as_ref().context("rename_file requires `file`")?;
            let raw_dest = input
                .new_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("rename_file requires `new_name` (sibling filename or relative path)")?;
            let from_abs = resolve_file_in_root(&root, file)?;
            let from_display = display_relative(&root, &from_abs);
            let dest_input = if !raw_dest.contains('/') && !raw_dest.contains('\\') {
                from_abs
                    .parent()
                    .unwrap_or(&root)
                    .join(raw_dest)
            } else {
                PathBuf::from(raw_dest)
            };
            let to_abs = resolve_new_path_in_root(&root, &dest_input)?;
            let to_display = display_relative(&root, &to_abs);
            ensure!(!to_abs.exists(), "destination already exists: {to_display}");

            let old_uri = path_to_file_uri(&from_abs)?;
            let new_uri = path_to_file_uri(&to_abs)?;
            let candidates = select_servers(&defs, &root, Some(&from_abs), "rename_file");
            let mut ws_edits = Vec::new();
            let mut servers_used = Vec::new();
            let mut clients_used = Vec::new();

            for srv in candidates {
                let Ok(client) = get_or_start_server(pool, &defs, &root, &srv, timeout).await else {
                    continue;
                };
                if !client.supports_will_rename_files().await {
                    continue;
                }
                let _ = client.reconcile_open_files().await;
                let _ = client.sync_file(&from_abs).await;
                client.wait_for_index(timeout.min(Duration::from_secs(10))).await;
                if let Ok(edit_val) = client
                    .request(
                        "workspace/willRenameFiles",
                        json!({ "files": [{ "oldUri": old_uri, "newUri": new_uri }] }),
                        timeout,
                    )
                    .await
                {
                    servers_used.push(srv.def.name.clone());
                    ws_edits.push(edit_val);
                    clients_used.push(client);
                }
            }

            let by_uri = collect_workspace_edits(&ws_edits)?;
            let apply = input.apply.unwrap_or(false);
            let mut res = apply_or_preview_workspace_edits(
                &root,
                by_uri,
                Some(FileMove {
                    from_abs: from_abs.clone(),
                    to_abs: to_abs.clone(),
                    from_display,
                    to_display,
                }),
                apply,
            )?;
            res["servers"] = json!(servers_used);
            if apply {
                for client in clients_used {
                    client.close_uri(&old_uri).await;
                    let _ = client
                        .notify(
                            "workspace/didRenameFiles",
                            json!({ "files": [{ "oldUri": old_uri, "newUri": new_uri }] }),
                        )
                        .await;
                    let _ = client.reconcile_open_files().await;
                }
            }
            Ok(res)
        }
        other => bail!(
            "unknown lsp action '{other}'; expected status, diagnostics, definition, references, hover, type_definition, implementation, symbols, workspace_symbols, code_actions, rename, rename_file, or shutdown"
        ),
    }
}

fn is_empty_lsp_result(val: &Value) -> bool {
    match val {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

async fn get_or_start_server(
    pool: &LspPool,
    defs: &[ServerDef],
    root: &Path,
    srv: &ResolvedServer,
    timeout: Duration,
) -> Result<std::sync::Arc<LspClient>> {
    if srv.def.name == "vue-language-server" {
        let bridge = match resolve_vue_ts_companion(defs, root, &srv.workspace_root) {
            Some(companion) => pool.get_or_start(&companion, timeout).await.ok(),
            None => None,
        };
        return pool.get_or_start_with_bridge(srv, timeout, bridge).await;
    }
    pool.get_or_start(srv, timeout).await
}

fn format_diagnostic(server: &str, item: &Value) -> Value {
    let sev = match item.get("severity").and_then(Value::as_u64) {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "diagnostic",
    };
    let start_line = item.pointer("/range/start/line").and_then(Value::as_u64).map(|n| n + 1);
    let start_col = item.pointer("/range/start/character").and_then(Value::as_u64).map(|n| n + 1);
    let end_line = item.pointer("/range/end/line").and_then(Value::as_u64).map(|n| n + 1);
    json!({
        "server": server,
        "severity": sev,
        "line": start_line,
        "column": start_col,
        "end_line": end_line,
        "code": item.get("code"),
        "source": item.get("source"),
        "message": item.get("message"),
    })
}

fn simplify_locations(root: &Path, val: Value) -> Value {
    match val {
        Value::Null => Value::Array(Vec::new()),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| simplify_location_item(root, item))
                .collect(),
        ),
        single => Value::Array(vec![simplify_location_item(root, single)]),
    }
}

fn simplify_location_item(root: &Path, item: Value) -> Value {
    let uri = item
        .get("uri")
        .or_else(|| item.get("targetUri"))
        .or_else(|| item.pointer("/location/uri"))
        .and_then(Value::as_str);
    let range = item
        .get("range")
        .or_else(|| item.get("targetSelectionRange"))
        .or_else(|| item.get("targetRange"))
        .or_else(|| item.pointer("/location/range"));

    if let (Some(uri), Some(range)) = (uri, range) {
        let path = file_uri_to_path(uri)
            .map(|p| display_relative(root, &p))
            .unwrap_or_else(|_| uri.to_owned());
        let mut obj = json!({
            "path": path,
            "start_line": range.pointer("/start/line").and_then(Value::as_u64).map(|n| n + 1),
            "start_column": range.pointer("/start/character").and_then(Value::as_u64).map(|n| n + 1),
            "end_line": range.pointer("/end/line").and_then(Value::as_u64).map(|n| n + 1),
        });
        if let Some(name) = item.get("name") {
            obj["name"] = name.clone();
        }
        if let Some(kind) = item.get("kind") {
            obj["kind"] = kind.clone();
        }
        if let Some(container) = item.get("containerName") {
            obj["container"] = container.clone();
        }
        return obj;
    }
    item
}
