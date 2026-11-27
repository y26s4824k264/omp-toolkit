//! Persistent multi-session DAP debug manager held inside the MCP server.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::dap::{
    client::{DapClient, SessionState},
    config::ResolvedAdapter,
};

#[derive(Debug, Clone)]
pub struct BreakpointEntry {
    pub line: u32,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
}

pub struct DebugSession {
    pub id: String,
    pub cwd: PathBuf,
    pub target: String,
    pub client: Arc<DapClient>,
    breakpoints: Mutex<BTreeMap<PathBuf, Vec<BreakpointEntry>>>,
}

impl DebugSession {
    pub async fn snapshot(&self) -> Value {
        let state = self.client.shared.state.lock().await.clone();
        let stopped = self.client.shared.stopped.lock().await.clone();
        let exit_code = *self.client.shared.exit_code.lock().await;
        let bp_count: usize = self
            .breakpoints
            .lock()
            .await
            .values()
            .map(Vec::len)
            .sum();
        json!({
            "session_id": self.id,
            "adapter": self.client.adapter.id,
            "cwd": self.cwd,
            "target": self.target,
            "status": state.as_str(),
            "thread_id": stopped.thread_id,
            "stop_reason": if state == SessionState::Stopped { stopped.reason } else { None },
            "stop_description": if state == SessionState::Stopped { stopped.description } else { None },
            "exit_code": exit_code,
            "breakpoint_count": bp_count,
        })
    }

    async fn resolve_thread_id(&self, req_timeout: Duration) -> Result<i64> {
        if let Some(tid) = self.client.shared.stopped.lock().await.thread_id {
            return Ok(tid);
        }
        let body = self
            .client
            .request("threads", json!({}), req_timeout)
            .await?;
        body.get("threads")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|t| t.get("id"))
            .and_then(Value::as_i64)
            .context("debug adapter reported no active threads")
    }

    pub async fn set_breakpoint(
        &self,
        file: PathBuf,
        line: u32,
        condition: Option<String>,
        hit_condition: Option<String>,
        req_timeout: Duration,
    ) -> Result<Value> {
        let mut map = self.breakpoints.lock().await;
        let list = map.entry(file.clone()).or_default();
        list.retain(|bp| bp.line != line);
        list.push(BreakpointEntry {
            line,
            condition,
            hit_condition,
        });
        list.sort_by_key(|bp| bp.line);
        let payload = build_breakpoints_payload(&file, list);
        drop(map);
        let body = self
            .client
            .request("setBreakpoints", payload, req_timeout)
            .await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "file": file,
            "breakpoints": body.get("breakpoints").cloned().unwrap_or(Value::Array(vec![])),
        }))
    }

    pub async fn remove_breakpoint(
        &self,
        file: PathBuf,
        line: Option<u32>,
        req_timeout: Duration,
    ) -> Result<Value> {
        let mut map = self.breakpoints.lock().await;
        let list = map.entry(file.clone()).or_default();
        if let Some(target_line) = line {
            list.retain(|bp| bp.line != target_line);
        } else {
            list.clear();
        }
        let payload = build_breakpoints_payload(&file, list);
        if list.is_empty() {
            map.remove(&file);
        }
        drop(map);
        let body = self
            .client
            .request("setBreakpoints", payload, req_timeout)
            .await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "file": file,
            "breakpoints": body.get("breakpoints").cloned().unwrap_or(Value::Array(vec![])),
        }))
    }

    pub async fn resume(&self, dap_command: &str, req_timeout: Duration) -> Result<Value> {
        {
            let st = self.client.shared.state.lock().await;
            ensure!(
                *st == SessionState::Stopped,
                "debuggee must be stopped before `{dap_command}` (currently {})",
                st.as_str()
            );
        }
        let thread_id = self.resolve_thread_id(req_timeout).await?;
        *self.client.shared.state.lock().await = SessionState::Running;
        if let Err(err) = self
            .client
            .request(dap_command, json!({ "threadId": thread_id }), req_timeout)
            .await
        {
            *self.client.shared.state.lock().await = SessionState::Stopped;
            return Err(err);
        }
        let settled = self.client.wait_for_stop(req_timeout).await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "timed_out_running": !settled,
        }))
    }

    pub async fn pause(&self, req_timeout: Duration) -> Result<Value> {
        let running = *self.client.shared.state.lock().await == SessionState::Running;
        if running {
            let thread_id = self.resolve_thread_id(req_timeout).await?;
            self.client
                .request("pause", json!({ "threadId": thread_id }), req_timeout)
                .await?;
            let _ = self.client.wait_for_stop(req_timeout).await?;
        }
        Ok(json!({ "snapshot": self.snapshot().await }))
    }

    pub async fn threads(&self, req_timeout: Duration) -> Result<Value> {
        let body = self.client.request("threads", json!({}), req_timeout).await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "threads": body.get("threads").cloned().unwrap_or(Value::Array(vec![])),
        }))
    }

    pub async fn stack_trace(&self, levels: u32, req_timeout: Duration) -> Result<Value> {
        self.ensure_stopped().await?;
        let thread_id = self.resolve_thread_id(req_timeout).await?;
        let body = self
            .client
            .request(
                "stackTrace",
                json!({ "threadId": thread_id, "startFrame": 0, "levels": levels }),
                req_timeout,
            )
            .await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "stack_frames": body.get("stackFrames").cloned().unwrap_or(Value::Array(vec![])),
            "total_frames": body.get("totalFrames"),
        }))
    }

    pub async fn scopes(&self, frame_id: i64, req_timeout: Duration) -> Result<Value> {
        self.ensure_stopped().await?;
        let body = self
            .client
            .request("scopes", json!({ "frameId": frame_id }), req_timeout)
            .await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "scopes": body.get("scopes").cloned().unwrap_or(Value::Array(vec![])),
        }))
    }

    pub async fn variables(&self, variable_ref: i64, req_timeout: Duration) -> Result<Value> {
        self.ensure_stopped().await?;
        let body = self
            .client
            .request(
                "variables",
                json!({ "variablesReference": variable_ref }),
                req_timeout,
            )
            .await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "variables": body.get("variables").cloned().unwrap_or(Value::Array(vec![])),
        }))
    }

    pub async fn evaluate(
        &self,
        expression: &str,
        frame_id: Option<i64>,
        context: Option<&str>,
        req_timeout: Duration,
    ) -> Result<Value> {
        self.ensure_stopped().await?;
        let mut args = Map::new();
        args.insert("expression".into(), json!(expression));
        args.insert("context".into(), json!(context.unwrap_or("repl")));
        if let Some(fid) = frame_id {
            args.insert("frameId".into(), json!(fid));
        }
        let body = self
            .client
            .request("evaluate", Value::Object(args), req_timeout)
            .await?;
        Ok(json!({
            "snapshot": self.snapshot().await,
            "result": body.get("result"),
            "type": body.get("type"),
            "variables_reference": body.get("variablesReference"),
        }))
    }

    pub async fn drain_output(&self, clear: bool) -> Value {
        let mut ring = self.client.shared.output_ring.lock().await;
        let text = if clear {
            std::mem::take(&mut *ring)
        } else {
            ring.clone()
        };
        json!({
            "snapshot": self.snapshot().await,
            "output": text,
        })
    }

    async fn ensure_stopped(&self) -> Result<()> {
        let st = self.client.shared.state.lock().await;
        ensure!(
            *st == SessionState::Stopped,
            "pause the debuggee first (currently {})",
            st.as_str()
        );
        Ok(())
    }
}

fn build_breakpoints_payload(file: &Path, entries: &[BreakpointEntry]) -> Value {
    let bps: Vec<Value> = entries
        .iter()
        .map(|bp| {
            let mut obj = Map::new();
            obj.insert("line".into(), json!(bp.line));
            if let Some(ref cond) = bp.condition {
                obj.insert("condition".into(), json!(cond));
            }
            if let Some(ref hit) = bp.hit_condition {
                obj.insert("hitCondition".into(), json!(hit));
            }
            Value::Object(obj)
        })
        .collect();
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("source");
    json!({
        "source": { "path": file.to_string_lossy(), "name": name },
        "breakpoints": bps,
    })
}

#[derive(Default)]
pub struct DapManager {
    sessions: Mutex<HashMap<String, Arc<DebugSession>>>,
    next_id: AtomicU64,
}

impl DapManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn list_sessions(&self) -> Vec<Value> {
        let list: Vec<Arc<DebugSession>> = self.sessions.lock().await.values().cloned().collect();
        let mut out = Vec::with_capacity(list.len());
        for s in list {
            out.push(s.snapshot().await);
        }
        out
    }

    pub async fn get_session(&self, session_id: Option<&str>) -> Result<Arc<DebugSession>> {
        let map = self.sessions.lock().await;
        if let Some(id) = session_id.map(str::trim).filter(|s| !s.is_empty()) {
            return map
                .get(id)
                .cloned()
                .with_context(|| format!("debug session '{id}' not found"));
        }
        match map.len() {
            0 => bail!("no active debug session; run `launch` or `attach` first"),
            1 => Ok(Arc::clone(map.values().next().unwrap())),
            n => bail!("multiple ({n}) debug sessions are active; pass `session_id`"),
        }
    }

    pub async fn launch(
        &self,
        adapter: ResolvedAdapter,
        cwd: PathBuf,
        program: PathBuf,
        program_args: Vec<String>,
        stop_on_entry: bool,
        req_timeout: Duration,
    ) -> Result<Arc<DebugSession>> {
        let client = DapClient::connect(adapter, &cwd, req_timeout).await?;
        let mut launch_args = client.adapter.launch_defaults.clone();
        launch_args.insert("program".into(), json!(program.to_string_lossy()));
        launch_args.insert("cwd".into(), json!(cwd.to_string_lossy()));
        launch_args.insert("args".into(), json!(program_args));
        launch_args.insert("stopOnEntry".into(), json!(stop_on_entry));
        if client.adapter.id == "debugpy" {
            launch_args.insert("console".into(), json!("internalConsole"));
        }

        if let Err(err) = initialize_and_start(&client, "launch", Value::Object(launch_args), req_timeout).await {
            client.shutdown().await;
            return Err(err);
        }

        let id = format!("dbg-{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let session = Arc::new(DebugSession {
            id: id.clone(),
            cwd,
            target: program.to_string_lossy().into_owned(),
            client,
            breakpoints: Mutex::new(BTreeMap::new()),
        });
        self.sessions.lock().await.insert(id, Arc::clone(&session));
        Ok(session)
    }

    pub async fn attach(
        &self,
        adapter: ResolvedAdapter,
        cwd: PathBuf,
        pid: Option<u32>,
        host: Option<String>,
        port: Option<u16>,
        req_timeout: Duration,
    ) -> Result<Arc<DebugSession>> {
        ensure!(
            pid.is_some() || port.is_some(),
            "attach requires `pid` or `port`"
        );
        let client = DapClient::connect(adapter, &cwd, req_timeout).await?;
        let mut attach_args = client.adapter.attach_defaults.clone();
        attach_args.insert("cwd".into(), json!(cwd.to_string_lossy()));
        let target_label = if let Some(pid) = pid {
            attach_args.insert("pid".into(), json!(pid));
            attach_args.insert("processId".into(), json!(pid));
            format!("pid:{pid}")
        } else {
            let h = host.unwrap_or_else(|| "127.0.0.1".into());
            let p = port.unwrap();
            attach_args.insert("host".into(), json!(h));
            attach_args.insert("port".into(), json!(p));
            format!("{h}:{p}")
        };

        if let Err(err) = initialize_and_start(&client, "attach", Value::Object(attach_args), req_timeout).await {
            client.shutdown().await;
            return Err(err);
        }

        let id = format!("dbg-{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let session = Arc::new(DebugSession {
            id: id.clone(),
            cwd,
            target: target_label,
            client,
            breakpoints: Mutex::new(BTreeMap::new()),
        });
        self.sessions.lock().await.insert(id, Arc::clone(&session));
        Ok(session)
    }

    pub async fn terminate(&self, session_id: Option<&str>) -> Result<Value> {
        let session = self.get_session(session_id).await?;
        let id = session.id.clone();
        self.sessions.lock().await.remove(&id);
        session.client.shutdown().await;
        Ok(json!({
            "terminated": id,
            "snapshot": session.snapshot().await,
        }))
    }

    pub async fn shutdown_all(&self) {
        let all: Vec<Arc<DebugSession>> = {
            let mut map = self.sessions.lock().await;
            map.drain().map(|(_, s)| s).collect()
        };
        for session in all {
            session.client.shutdown().await;
        }
    }
}

async fn initialize_and_start(
    client: &Arc<DapClient>,
    start_cmd: &str,
    start_args: Value,
    req_timeout: Duration,
) -> Result<()> {
    let caps = client
        .request(
            "initialize",
            json!({
                "clientID": "omp-toolkit",
                "clientName": "OMP Toolkit",
                "adapterID": client.adapter.id,
                "pathFormat": "path",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "supportsVariableType": true,
            }),
            req_timeout,
        )
        .await?;

    let supports_config_done = caps
        .get("supportsConfigurationDoneRequest")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if supports_config_done {
        let start_fut = client.request(start_cmd, start_args, req_timeout);
        let config_fut = async {
            client.wait_for_initialized(req_timeout).await?;
            let has_ex = caps
                .get("supportsExceptionFilterOptions")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || caps
                    .get("exceptionBreakpointFilters")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty());
            if has_ex {
                let _ = client
                    .request("setExceptionBreakpoints", json!({ "filters": [] }), req_timeout)
                    .await;
            }
            client
                .request("configurationDone", json!({}), req_timeout)
                .await?;
            Ok::<(), anyhow::Error>(())
        };
        let (start_res, cfg_res) = tokio::join!(start_fut, config_fut);
        start_res?;
        cfg_res?;
    } else {
        client.request(start_cmd, start_args, req_timeout).await?;
    }

    {
        let mut st = client.shared.state.lock().await;
        if *st == SessionState::Launching {
            *st = SessionState::Running;
        }
    }
    let _ = client.wait_for_stop(Duration::from_secs(3)).await?;
    Ok(())
}
