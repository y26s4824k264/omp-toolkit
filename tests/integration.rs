//! End-to-end integration tests driving the real `omp-toolkit mcp` binary over stdio,
//! including real persistent LSP & DAP protocol exchanges against lightweight mock servers.

use std::{
    fs,
    path::Path,
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

struct McpHarness {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpHarness {
    async fn spawn() -> Result<Self> {
        let bin = env!("CARGO_BIN_EXE_omp-toolkit");
        let mut child = Command::new(bin)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("spawn omp-toolkit mcp")?;
        let stdin = child.stdin.take().context("stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("stdout")?);
        let mut harness = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let init = harness
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "integration-test", "version": "1.0" }
                }),
            )
            .await?;
        assert_eq!(init["serverInfo"]["name"], "omp-toolkit");
        harness.notify("notifications/initialized", json!({})).await?;
        Ok(harness)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.stdin
            .write_all(format!("{}\n", serde_json::to_string(&msg)?).as_bytes())
            .await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.stdin
            .write_all(format!("{}\n", serde_json::to_string(&msg)?).as_bytes())
            .await?;
        self.stdin.flush().await?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = timeout(Duration::from_secs(15), self.stdout.read_line(&mut line))
                .await
                .context("MCP response timed out")??;
            anyhow::ensure!(n > 0, "MCP server exited before responding to {method}");
            let reply: Value = serde_json::from_str(&line)?;
            if reply.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(err) = reply.get("error") {
                anyhow::bail!("MCP JSON-RPC error: {err}");
            }
            return Ok(reply["result"].clone());
        }
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String> {
        let res = self
            .request("tools/call", json!({ "name": name, "arguments": arguments }))
            .await?;
        if res.get("isError").and_then(Value::as_bool) == Some(true) {
            anyhow::bail!("tool {name} returned error: {}", res["content"]);
        }
        Ok(res["content"][0]["text"]
            .as_str()
            .context("tool result missing text")?
            .to_owned())
    }

    async fn close(mut self) -> Result<()> {
        drop(self.stdin);
        let status = timeout(Duration::from_secs(5), self.child.wait()).await??;
        assert!(status.success(), "MCP exited with {status}");
        Ok(())
    }
}

fn python_executable() -> String {
    for candidate in ["python3", "python"] {
        if let Ok(out) = std::process::Command::new(candidate)
            .arg("--version")
            .output()
            && out.status.success()
        {
            return candidate.to_owned();
        }
    }
    "python3".to_owned()
}

fn write_mock_lsp(dir: &Path) -> Result<()> {
    let script = r#"#!/usr/bin/env python3
import json, sys, os

pid_file = os.path.join(os.getcwd(), ".mock-lsp-starts")
with open(pid_file, "a") as f:
    f.write(f"{os.getpid()}\n")

docs = {}

def read_msg():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        k, v = line.decode("ascii").split(":", 1)
        headers[k.strip().lower()] = v.strip()
    length = int(headers["content-length"])
    return json.loads(sys.stdin.buffer.read(length).decode("utf-8"))

def send(obj):
    body = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    if msg is None:
        break
    method = msg.get("method")
    req_id = msg.get("id")
    params = msg.get("params") or {}

    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "capabilities": {
                    "textDocumentSync": 1,
                    "definitionProvider": True,
                    "renameProvider": True,
                    "diagnosticProvider": True
                }
            }
        })
    elif method == "textDocument/didOpen":
        td = params["textDocument"]
        docs[td["uri"]] = td["text"]
    elif method == "textDocument/didChange":
        uri = params["textDocument"]["uri"]
        docs[uri] = params["contentChanges"][-1]["text"]
    elif method == "textDocument/diagnostic":
        uri = params["textDocument"]["uri"]
        text = docs.get(uri, "")
        items = []
        if "BROKEN" in text:
            items.append({
                "severity": 1,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 6}},
                "message": "found BROKEN marker"
            })
        send({"jsonrpc": "2.0", "id": req_id, "result": {"kind": "full", "items": items}})
    elif method == "textDocument/definition":
        uri = params["textDocument"]["uri"]
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "uri": uri,
                "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 8}}
            }
        })
    elif method == "textDocument/rename":
        uri = params["textDocument"]["uri"]
        new_name = params["newName"]
        text = docs.get(uri, "")
        idx = text.find("alpha")
        edits = []
        if idx >= 0:
            edits.append({
                "range": {"start": {"line": 0, "character": idx}, "end": {"line": 0, "character": idx + 5}},
                "newText": new_name
            })
        send({"jsonrpc": "2.0", "id": req_id, "result": {"changes": {uri: edits}}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": req_id, "result": None})
    elif method == "exit":
        break
"#;
    let script_path = dir.join("mock_lsp.py");
    fs::write(&script_path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(
        dir.join(".lsp.json"),
        serde_json::to_string_pretty(&json!({
            "servers": {
                "mock-lsp": {
                    "command": python_executable(),
                    "args": [script_path.to_string_lossy()],
                    "fileTypes": [".mock"],
                    "rootMarkers": [".lsp.json"]
                }
            }
        }))?,
    )?;
    Ok(())
}

fn write_mock_dap(dir: &Path) -> Result<()> {
    let script = r#"#!/usr/bin/env python3
import json, sys

seq = 0
def read_msg():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        k, v = line.decode("ascii").split(":", 1)
        headers[k.strip().lower()] = v.strip()
    length = int(headers["content-length"])
    return json.loads(sys.stdin.buffer.read(length).decode("utf-8"))

def send(obj):
    global seq
    seq += 1
    obj["seq"] = seq
    body = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    if msg is None:
        break
    cmd = msg.get("command")
    rseq = msg.get("seq")
    args = msg.get("arguments") or {}

    if cmd == "initialize":
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True,
              "body": {"supportsConfigurationDoneRequest": True}})
        send({"type": "event", "event": "initialized", "body": {}})
    elif cmd == "launch":
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True, "body": {}})
        send({"type": "event", "event": "output", "body": {"category": "stdout", "output": "hello from debuggee\n"}})
        send({"type": "event", "event": "stopped", "body": {"reason": "entry", "threadId": 1}})
    elif cmd == "configurationDone":
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True, "body": {}})
    elif cmd == "setBreakpoints":
        bps = [{"verified": True, "line": bp["line"]} for bp in args.get("breakpoints", [])]
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True, "body": {"breakpoints": bps}})
    elif cmd == "threads":
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True,
              "body": {"threads": [{"id": 1, "name": "main"}]}})
    elif cmd == "stackTrace":
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True,
              "body": {"stackFrames": [{"id": 10, "name": "main", "line": 4, "column": 1}], "totalFrames": 1}})
    elif cmd == "evaluate":
        expr = args.get("expression", "")
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True,
              "body": {"result": f"eval({expr})=42", "type": "int", "variablesReference": 0}})
    elif cmd == "disconnect":
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True, "body": {}})
        break
    else:
        send({"type": "response", "request_seq": rseq, "command": cmd, "success": True, "body": {}})
"#;
    let script_path = dir.join("mock_dap.py");
    fs::write(&script_path, script)?;
    fs::write(
        dir.join(".dap.json"),
        serde_json::to_string_pretty(&json!({
            "adapters": {
                "mock-dap": {
                    "command": python_executable(),
                    "args": [script_path.to_string_lossy()],
                    "languages": ["mock"],
                    "fileTypes": [".mock"]
                }
            }
        }))?,
    )?;
    Ok(())
}

#[tokio::test]
async fn mcp_exposes_lean_8_tool_surface_and_runs_persistent_lsp_and_dap() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    fs::write(
        root.join("demo.rs"),
        "pub fn compute(x: i32) -> i32 {\n    let a = x + 1;\n    let b = a * 2;\n    let c = b + 3;\n    let d = c + 4;\n    old_fn(d)\n}\n",
    )?;
    fs::write(
        root.join("conflict.rs"),
        "fn main() {\n<<<<<<< HEAD\n    let v = 1;\n||||||| base\n    let v = 0;\n=======\n    let v = 2;\n>>>>>>> feat\n}\n",
    )?;
    fs::write(root.join("sample.mock"), "fn alpha() {}\n")?;
    write_mock_lsp(&root)?;
    write_mock_dap(&root)?;

    let mut mcp = McpHarness::spawn().await?;

    // 1. Verify exact lean 8-tool surface
    let list = mcp.request("tools/list", json!({})).await?;
    let mut names: Vec<&str> = list["tools"]
        .as_array()
        .context("tools array")?
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "ast_edit",
            "ast_grep",
            "code_outline",
            "code_summary",
            "debug",
            "git_conflicts",
            "lsp",
            "minimize_output",
        ]
    );

    // 2. code_summary (folded + syntactic block_line expansion) & code_outline
    let summary = mcp
        .call_tool(
            "code_summary",
            json!({ "root": root, "file": "demo.rs", "unfold_until_lines": 0, "min_body_lines": 3 }),
        )
        .await?;
    assert!(summary.contains("elided"), "{summary}");

    let block_view = mcp
        .call_tool(
            "code_summary",
            json!({ "root": root, "file": "demo.rs", "block_line": 1 }),
        )
        .await?;
    assert!(block_view.contains("demo.rs:L1-L7"), "{block_view}");
    assert!(block_view.contains("old_fn(d)"), "{block_view}");

    let outline: Value = serde_json::from_str(
        &mcp.call_tool("code_outline", json!({ "root": root, "file": "demo.rs" }))
            .await?,
    )?;
    assert_eq!(outline["items"][0]["name"], "compute");

    // 3. Full-repo ast_grep & ast_edit (dry-run then apply)
    let grep_res: Value = serde_json::from_str(
        &mcp.call_tool(
            "ast_grep",
            json!({ "root": root, "patterns": ["old_fn($X)"] }),
        )
        .await?,
    )?;
    assert_eq!(grep_res["total_matches"], 1);

    let dry: Value = serde_json::from_str(
        &mcp.call_tool(
            "ast_edit",
            json!({
                "root": root,
                "rules": [{ "pattern": "old_fn($X)", "rewrite": "new_fn($X)" }]
            }),
        )
        .await?,
    )?;
    assert_eq!(dry["applied"], false);
    assert!(dry["diff"].as_str().unwrap().contains("+    new_fn(d)"));
    assert!(fs::read_to_string(root.join("demo.rs"))?.contains("old_fn(d)"));

    let applied: Value = serde_json::from_str(
        &mcp.call_tool(
            "ast_edit",
            json!({
                "root": root,
                "rules": [{ "pattern": "old_fn($X)", "rewrite": "new_fn($X)" }],
                "apply": true
            }),
        )
        .await?,
    )?;
    assert_eq!(applied["applied"], true);
    assert!(fs::read_to_string(root.join("demo.rs"))?.contains("new_fn(d)"));

    // 4. Persistent LSP: warm reuse + external disk edit auto-sync + cross-platform rename preview/apply
    let diag1: Value = serde_json::from_str(
        &mcp.call_tool(
            "lsp",
            json!({ "action": "diagnostics", "root": root, "file": "sample.mock" }),
        )
        .await?,
    )?;
    assert_eq!(diag1["reports"][0]["error_count"], 0);

    // Simulate external editor (e.g. Claude Code / Codex) modifying the file directly on disk!
    fs::write(root.join("sample.mock"), "BROKEN fn alpha() {}\n")?;
    let diag2: Value = serde_json::from_str(
        &mcp.call_tool(
            "lsp",
            json!({ "action": "diagnostics", "root": root, "file": "sample.mock" }),
        )
        .await?,
    )?;
    assert_eq!(diag2["reports"][0]["error_count"], 1);

    // Confirm mock LSP was started ONLY ONCE across multiple tool calls (persistent pool!)
    let starts = fs::read_to_string(root.join(".mock-lsp-starts"))?;
    assert_eq!(starts.lines().count(), 1, "LSP server must stay warm and reused");

    // Rename preview -> apply
    let rename_preview: Value = serde_json::from_str(
        &mcp.call_tool(
            "lsp",
            json!({
                "action": "rename",
                "root": root,
                "file": "sample.mock",
                "symbol": "alpha",
                "new_name": "beta"
            }),
        )
        .await?,
    )?;
    assert_eq!(rename_preview["applied"], false);
    assert!(rename_preview["diff"].as_str().unwrap().contains("beta"));

    let rename_apply: Value = serde_json::from_str(
        &mcp.call_tool(
            "lsp",
            json!({
                "action": "rename",
                "root": root,
                "file": "sample.mock",
                "symbol": "alpha",
                "new_name": "beta",
                "apply": true
            }),
        )
        .await?,
    )?;
    assert_eq!(rename_apply["applied"], true);
    assert!(fs::read_to_string(root.join("sample.mock"))?.contains("fn beta()"));

    // 5. Persistent DAP: launch -> breakpoint -> stack_trace -> evaluate -> output -> terminate
    let launch: Value = serde_json::from_str(
        &mcp.call_tool(
            "debug",
            json!({
                "action": "launch",
                "root": root,
                "program": "sample.mock",
                "adapter": "mock-dap"
            }),
        )
        .await?,
    )?;
    assert_eq!(launch["snapshot"]["status"], "stopped");

    let bp: Value = serde_json::from_str(
        &mcp.call_tool(
            "debug",
            json!({ "action": "set_breakpoint", "root": root, "file": "sample.mock", "line": 1 }),
        )
        .await?,
    )?;
    assert_eq!(bp["breakpoints"][0]["verified"], true);

    let eval_res: Value = serde_json::from_str(
        &mcp.call_tool("debug", json!({ "action": "evaluate", "expression": "x + 1" }))
            .await?,
    )?;
    assert_eq!(eval_res["result"], "eval(x + 1)=42");

    let out_res: Value = serde_json::from_str(
        &mcp.call_tool("debug", json!({ "action": "output", "clear": true }))
            .await?,
    )?;
    assert!(out_res["output"].as_str().unwrap().contains("hello from debuggee"));

    let term: Value = serde_json::from_str(
        &mcp.call_tool("debug", json!({ "action": "terminate" })).await?,
    )?;
    assert!(term["terminated"].as_str().unwrap().starts_with("dbg-"));

    // 6. minimize_output (with non-zero exit code)
    let min_res: Value = serde_json::from_str(
        &mcp.call_tool(
            "minimize_output",
            json!({
                "command": "cargo test",
                "exit_code": 101,
                "output": "running 2 tests\ntest ok_one ... ok\ntest bad_one ... FAILED\n\nfailures:\n    bad_one\n\ntest result: FAILED. 1 passed; 1 failed\n"
            }),
        )
        .await?,
    )?;
    assert!(min_res["text"].as_str().unwrap().contains("bad_one"));

    // 7. git_conflicts (repo scan + dry-run preview + atomic apply)
    let conflict_scan: Value = serde_json::from_str(
        &mcp.call_tool("git_conflicts", json!({ "action": "scan", "root": root }))
            .await?,
    )?;
    assert_eq!(conflict_scan["files_with_conflicts"], 1);
    assert_eq!(conflict_scan["total_conflicts"], 1);
    assert_eq!(conflict_scan["files"][0]["path"], "conflict.rs");

    let conflict_preview: Value = serde_json::from_str(
        &mcp.call_tool(
            "git_conflicts",
            json!({
                "action": "resolve",
                "root": root,
                "file": "conflict.rs",
                "index": 1,
                "choice": "custom",
                "custom_text": "@ours\n    let merged = true;\n@theirs"
            }),
        )
        .await?,
    )?;
    assert_eq!(conflict_preview["applied"], false);
    assert_eq!(conflict_preview["remaining_conflicts"], 0);
    assert!(conflict_preview["diff"].as_str().unwrap().contains("+    let merged = true;"));

    let conflict_apply: Value = serde_json::from_str(
        &mcp.call_tool(
            "git_conflicts",
            json!({
                "action": "resolve",
                "root": root,
                "file": "conflict.rs",
                "index": 1,
                "choice": "theirs",
                "apply": true
            }),
        )
        .await?,
    )?;
    assert_eq!(conflict_apply["applied"], true);
    assert_eq!(fs::read_to_string(root.join("conflict.rs"))?, "fn main() {\n    let v = 2;\n}\n");

    mcp.close().await?;
    Ok(())
}
