//! Persistent asynchronous JSON-RPC 2.0 LSP stdio client with automatic
//! disk-change reconciliation (`didOpen` / `didChange`), diagnostics caching,
//! and `$/progress` indexing awareness.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, Notify, oneshot},
    time::{sleep, timeout},
};

use crate::{
    lsp::config::{ResolvedServer, language_id},
    path_guard::{path_to_file_uri, read_utf8_file},
};

const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_DOC_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct OpenDocument {
    pub uri: String,
    pub version: i32,
    pub text: String,
}

#[derive(Debug, Clone)]
struct DiagnosticEntry {
    items: Vec<Value>,
    updated_at: Instant,
}

struct ClientShared {
    server: ResolvedServer,
    workspace_uri: String,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>,
    diagnostics: Mutex<HashMap<String, DiagnosticEntry>>,
    diag_notify: Notify,
    progress_tokens: Mutex<HashSet<String>>,
    progress_notify: Notify,
    dynamic_caps: Mutex<HashMap<String, String>>,
    stderr_tail: Mutex<String>,
    dead: AtomicBool,
}

pub struct LspClient {
    shared: Arc<ClientShared>,
    stdin: Arc<Mutex<ChildStdin>>,
    child: Mutex<Child>,
    next_id: AtomicI64,
    capabilities: Mutex<Value>,
    open_docs: Mutex<HashMap<String, OpenDocument>>,
}

impl LspClient {
    /// Spawn and initialize a persistent language server process.
    pub async fn start(server: ResolvedServer, init_timeout: Duration) -> Result<Arc<Self>> {
        let workspace_uri = path_to_file_uri(&server.workspace_root)?;
        let mut cmd = Command::new(&server.binary);
        cmd.args(&server.args)
            .current_dir(&server.workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "spawn language server '{}' ({})",
                server.def.name,
                server.binary.display()
            )
        })?;
        let stdin = child.stdin.take().context("capture LSP stdin")?;
        let stdout = child.stdout.take().context("capture LSP stdout")?;
        let stderr = child.stderr.take().context("capture LSP stderr")?;

        let shared = Arc::new(ClientShared {
            server,
            workspace_uri,
            pending: Mutex::new(HashMap::new()),
            diagnostics: Mutex::new(HashMap::new()),
            diag_notify: Notify::new(),
            progress_tokens: Mutex::new(HashSet::new()),
            progress_notify: Notify::new(),
            dynamic_caps: Mutex::new(HashMap::new()),
            stderr_tail: Mutex::new(String::new()),
            dead: AtomicBool::new(false),
        });
        let stdin = Arc::new(Mutex::new(stdin));

        // Capture stderr tail for diagnostics if the server crashes.
        {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = [0u8; 2048];
                while let Ok(n) = reader.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    let mut tail = shared.stderr_tail.lock().await;
                    tail.push_str(&chunk);
                    if tail.len() > 2048 {
                        let start = tail.floor_char_boundary(tail.len() - 2048);
                        tail.drain(..start);
                    }
                }
            });
        }

        // Background stdout JSON-RPC frame reader.
        {
            let shared = Arc::clone(&shared);
            let stdin = Arc::clone(&stdin);
            tokio::spawn(async move {
                let res = read_loop(BufReader::new(stdout), &shared, &stdin).await;
                shared.dead.store(true, Ordering::SeqCst);
                let reason = match res {
                    Ok(()) => format!("{} exited", shared.server.def.name),
                    Err(e) => format!("{} reader stopped: {e:#}", shared.server.def.name),
                };
                let tail = shared.stderr_tail.lock().await.trim().to_owned();
                let full_err = if tail.is_empty() {
                    reason
                } else {
                    format!("{reason}\n{tail}")
                };
                let mut pending = shared.pending.lock().await;
                for (_, tx) in pending.drain() {
                    let _ = tx.send(Err(anyhow!("{full_err}")));
                }
                shared.diag_notify.notify_waiters();
                shared.progress_notify.notify_waiters();
            });
        }

        let client = Arc::new(Self {
            shared,
            stdin,
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
            capabilities: Mutex::new(Value::Object(Default::default())),
            open_docs: Mutex::new(HashMap::new()),
        });

        client.initialize(init_timeout).await?;
        Ok(client)
    }

    pub fn is_dead(&self) -> bool {
        self.shared.dead.load(Ordering::SeqCst)
    }

    pub fn server_name(&self) -> &str {
        &self.shared.server.def.name
    }

    pub fn workspace_root(&self) -> &Path {
        &self.shared.server.workspace_root
    }

    pub async fn capabilities(&self) -> Value {
        self.capabilities.lock().await.clone()
    }

    pub async fn supports(&self, cap_key: &str, method: &str) -> bool {
        let caps = self.capabilities.lock().await;
        let static_ok = match caps.get(cap_key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Object(_)) => true,
            _ => false,
        };
        if static_ok {
            return true;
        }
        self.shared
            .dynamic_caps
            .lock()
            .await
            .values()
            .any(|m| m == method)
    }

    pub async fn supports_will_rename_files(&self) -> bool {
        let caps = self.capabilities.lock().await;
        let static_ok = caps
            .pointer("/workspace/fileOperations/willRename")
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false));
        if static_ok {
            return true;
        }
        self.shared
            .dynamic_caps
            .lock()
            .await
            .values()
            .any(|m| m == "workspace/willRenameFiles")
    }

    async fn initialize(&self, init_timeout: Duration) -> Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "rootPath": self.shared.server.workspace_root.to_string_lossy(),
            "rootUri": self.shared.workspace_uri,
            "workspaceFolders": [{
                "uri": self.shared.workspace_uri,
                "name": self.shared.server.workspace_root.to_string_lossy(),
            }],
            "clientInfo": { "name": "omp-toolkit", "version": env!("CARGO_PKG_VERSION") },
            "initializationOptions": self.shared.server.def.init_options,
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "window": { "workDoneProgress": true },
                "workspace": {
                    "applyEdit": false,
                    "configuration": true,
                    "workspaceFolders": true,
                    "workspaceEdit": { "documentChanges": true },
                    "fileOperations": {
                        "dynamicRegistration": true,
                        "willRename": true,
                        "didRename": true
                    }
                },
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "diagnostic": {},
                    "publishDiagnostics": { "relatedInformation": true },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "codeAction": {
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": ["quickfix", "refactor", "source"]
                            }
                        }
                    },
                    "rename": { "prepareSupport": false }
                }
            }
        });

        let reply = self.request("initialize", params, init_timeout).await?;
        let caps = reply
            .get("capabilities")
            .cloned()
            .unwrap_or_else(|| json!({}));
        *self.capabilities.lock().await = caps;
        self.notify("initialized", json!({})).await?;
        if !self.shared.server.def.settings.is_null() {
            self.notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": self.shared.server.def.settings }),
            )
            .await?;
        }
        Ok(())
    }

    /// Synchronize a file on disk into the language server, emitting `didOpen`
    /// on first access or `didChange` + `didSave` if external tools modified it.
    pub async fn sync_file(&self, abs_path: &Path) -> Result<OpenDocument> {
        let text = read_utf8_file(abs_path, MAX_DOC_BYTES)?;
        let uri = path_to_file_uri(abs_path)?;
        let mut docs = self.open_docs.lock().await;
        if let Some(existing) = docs.get_mut(&uri) {
            if existing.text == text {
                return Ok(existing.clone());
            }
            existing.version += 1;
            existing.text = text.clone();
            let doc = existing.clone();
            drop(docs);
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": doc.uri, "version": doc.version },
                    "contentChanges": [{ "text": doc.text }]
                }),
            )
            .await?;
            self.notify(
                "textDocument/didSave",
                json!({ "textDocument": { "uri": doc.uri } }),
            )
            .await?;
            return Ok(doc);
        }

        let lang_id = language_id(&self.shared.server.def, abs_path);
        let doc = OpenDocument {
            uri: uri.clone(),
            version: 1,
            text: text.clone(),
        };
        docs.insert(uri.clone(), doc.clone());
        drop(docs);
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": lang_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
        .await?;
        Ok(doc)
    }

    /// Re-sync every currently open file that changed on disk (crucial before cross-file queries).
    pub async fn reconcile_open_files(&self) -> Result<()> {
        let uris: Vec<String> = self.open_docs.lock().await.keys().cloned().collect();
        for uri in uris {
            let Ok(path) = crate::path_guard::file_uri_to_path(&uri) else {
                continue;
            };
            if path.is_file() {
                let _ = self.sync_file(&path).await;
            } else {
                self.close_uri(&uri).await;
            }
        }
        Ok(())
    }

    pub async fn close_uri(&self, uri: &str) {
        let removed = self.open_docs.lock().await.remove(uri).is_some();
        if removed {
            let _ = self
                .notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }))
                .await;
        }
    }

    pub async fn open_version(&self, uri: &str) -> Option<i32> {
        self.open_docs.lock().await.get(uri).map(|d| d.version)
    }

    /// Wait for any active `$/progress` indexing tasks to complete up to `budget`.
    pub async fn wait_for_index(&self, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            if self.is_dead() {
                return;
            }
            if self.shared.progress_tokens.lock().await.is_empty() {
                return;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            let _ = timeout(remaining.min(Duration::from_millis(120)), self.shared.progress_notify.notified()).await;
        }
    }

    /// Fetch diagnostics for `abs_path` using pull (`textDocument/diagnostic`) if supported,
    /// falling back to debounced push (`textDocument/publishDiagnostics`).
    pub async fn diagnostics(&self, abs_path: &Path, wait_budget: Duration) -> Result<Vec<Value>> {
        let doc = self.sync_file(abs_path).await?;
        let caps = self.capabilities.lock().await.clone();
        if caps.get("diagnosticProvider").is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
            && let Ok(reply) = self
                .request(
                    "textDocument/diagnostic",
                    json!({ "textDocument": { "uri": doc.uri } }),
                    wait_budget,
                )
                .await
            && let Some(items) = reply.get("items").and_then(Value::as_array)
        {
            return Ok(items.clone());
        }

        // Wait for push diagnostics to settle (debounce 180ms after latest update).
        let started = Instant::now();
        let deadline = started + wait_budget;
        loop {
            if self.is_dead() {
                bail!("{} exited while waiting for diagnostics", self.server_name());
            }
            {
                let map = self.shared.diagnostics.lock().await;
                if let Some(entry) = map.get(&doc.uri)
                    && entry.updated_at.elapsed() >= Duration::from_millis(180)
                {
                    return Ok(entry.items.clone());
                }
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                let map = self.shared.diagnostics.lock().await;
                return Ok(map
                    .get(&doc.uri)
                    .map(|e| e.items.clone())
                    .unwrap_or_default());
            };
            let _ = timeout(
                remaining.min(Duration::from_millis(80)),
                self.shared.diag_notify.notified(),
            )
            .await;
        }
    }

    pub async fn request(&self, method: &str, params: Value, req_timeout: Duration) -> Result<Value> {
        ensure!(!self.is_dead(), "{} is not running", self.server_name());
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().await.insert(id, tx);

        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = send_frame(&self.stdin, &payload).await {
            self.shared.pending.lock().await.remove(&id);
            return Err(err);
        }

        match timeout(req_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => bail!("{} connection closed during {method}", self.server_name()),
            Err(_) => {
                self.shared.pending.lock().await.remove(&id);
                let _ = self.notify("$/cancelRequest", json!({ "id": id })).await;
                bail!("{method} timed out for {}", self.server_name());
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        ensure!(!self.is_dead(), "{} is not running", self.server_name());
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        send_frame(&self.stdin, &payload).await
    }

    pub async fn shutdown(&self) {
        if !self.is_dead() {
            let _ = self
                .request("shutdown", Value::Null, Duration::from_secs(2))
                .await;
            let _ = self.notify("exit", Value::Null).await;
            sleep(Duration::from_millis(120)).await;
        }
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = timeout(Duration::from_millis(400), child.wait()).await;
    }
}

async fn send_frame(stdin: &Arc<Mutex<ChildStdin>>, payload: &Value) -> Result<()> {
    let body = serde_json::to_vec(payload)?;
    ensure!(
        body.len() <= MAX_PAYLOAD_BYTES,
        "LSP message exceeds {MAX_PAYLOAD_BYTES} bytes"
    );
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut writer = stdin.lock().await;
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_loop<R: tokio::io::AsyncRead + Unpin>(
    mut reader: BufReader<R>,
    shared: &Arc<ClientShared>,
    stdin: &Arc<Mutex<ChildStdin>>,
) -> Result<()> {
    let mut line = String::new();
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                content_length = Some(v.trim().parse::<usize>().context("parse Content-Length")?);
            }
        }
        let Some(len) = content_length else {
            continue;
        };
        ensure!(len > 0 && len <= MAX_PAYLOAD_BYTES, "invalid LSP frame length {len}");
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await?;
        let msg: Value = serde_json::from_slice(&body).context("parse LSP JSON message")?;
        dispatch_message(msg, shared, stdin).await?;
    }
}

async fn dispatch_message(
    msg: Value,
    shared: &Arc<ClientShared>,
    stdin: &Arc<Mutex<ChildStdin>>,
) -> Result<()> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str);

    match (id, method) {
        // Server -> Client request
        (Some(req_id), Some(method)) => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let response = match method {
                "workspace/configuration" => {
                    let items = params
                        .get("items")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let values: Vec<Value> = items
                        .iter()
                        .map(|item| {
                            let section = item.get("section").and_then(Value::as_str);
                            lookup_section(&shared.server.def.settings, section)
                        })
                        .collect();
                    json!({ "jsonrpc": "2.0", "id": req_id, "result": values })
                }
                "workspace/workspaceFolders" => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": [{
                            "uri": shared.workspace_uri,
                            "name": shared.server.workspace_root.to_string_lossy(),
                        }]
                    })
                }
                "workspace/applyEdit" => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": { "applied": false, "failureReason": "managed explicitly by omp-toolkit" }
                    })
                }
                "client/registerCapability" => {
                    if let Some(regs) = params.get("registrations").and_then(Value::as_array) {
                        let mut caps = shared.dynamic_caps.lock().await;
                        for reg in regs {
                            if let (Some(id), Some(m)) = (
                                reg.get("id").and_then(Value::as_str),
                                reg.get("method").and_then(Value::as_str),
                            ) {
                                caps.insert(id.to_owned(), m.to_owned());
                            }
                        }
                    }
                    json!({ "jsonrpc": "2.0", "id": req_id, "result": Value::Null })
                }
                "client/unregisterCapability" => {
                    let list = params
                        .get("unregisterations")
                        .or_else(|| params.get("unregistrations"))
                        .and_then(Value::as_array);
                    if let Some(unregs) = list {
                        let mut caps = shared.dynamic_caps.lock().await;
                        for item in unregs {
                            if let Some(id) = item.get("id").and_then(Value::as_str) {
                                caps.remove(id);
                            }
                        }
                    }
                    json!({ "jsonrpc": "2.0", "id": req_id, "result": Value::Null })
                }
                "window/workDoneProgress/create" | "window/showMessageRequest" => {
                    json!({ "jsonrpc": "2.0", "id": req_id, "result": Value::Null })
                }
                _ => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": { "code": -32601, "message": format!("unsupported client method {method}") }
                    })
                }
            };
            let _ = send_frame(stdin, &response).await;
        }
        // Response to our request
        (Some(req_id), None) => {
            if let Some(num_id) = req_id.as_i64()
                && let Some(tx) = shared.pending.lock().await.remove(&num_id)
            {
                if let Some(err) = msg.get("error") {
                    let message = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("LSP error");
                    let code = err.get("code").and_then(Value::as_i64).unwrap_or(-1);
                    let _ = tx.send(Err(anyhow!("LSP error ({code}): {message}")));
                } else {
                    let result = msg.get("result").cloned().unwrap_or(Value::Null);
                    let _ = tx.send(Ok(result));
                }
            }
        }
        // Server notification
        (None, Some(method)) => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "textDocument/publishDiagnostics" => {
                    if let (Some(uri), Some(items)) = (
                        params.get("uri").and_then(Value::as_str),
                        params.get("diagnostics").and_then(Value::as_array),
                    ) {
                        shared.diagnostics.lock().await.insert(
                            uri.to_owned(),
                            DiagnosticEntry {
                                items: items.clone(),
                                updated_at: Instant::now(),
                            },
                        );
                        shared.diag_notify.notify_waiters();
                    }
                }
                "$/progress" => {
                    let token = params.get("token").map(|v| v.to_string()).unwrap_or_default();
                    let kind = params.pointer("/value/kind").and_then(Value::as_str);
                    if let Some(kind) = kind {
                        let mut tokens = shared.progress_tokens.lock().await;
                        match kind {
                            "begin" => {
                                tokens.insert(token);
                            }
                            "end" => {
                                tokens.remove(&token);
                                shared.progress_notify.notify_waiters();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        (None, None) => {}
    }
    Ok(())
}

fn lookup_section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.clone();
    };
    let mut current = settings;
    for part in section.split('.') {
        match current.get(part) {
            Some(next) => current = next,
            None => return Value::Null,
        }
    }
    current.clone()
}
