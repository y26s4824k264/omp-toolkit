//! Asynchronous Debug Adapter Protocol (DAP) transport supporting both
//! stdio adapters (`lldb-dap`, `gdb -i dap`, `debugpy`) and TCP socket
//! adapters (`dlv dap`, `codelldb --port 0`).

use std::{
    collections::HashMap,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use regex::Regex;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
    sync::{Mutex, Notify, oneshot},
    time::timeout,
};

use crate::dap::config::ResolvedAdapter;

const MAX_DAP_BODY: usize = 8 * 1024 * 1024;
const MAX_OUTPUT_RING: usize = 64 * 1024;

#[derive(Debug, Clone, Default)]
pub struct StoppedState {
    pub reason: Option<String>,
    pub thread_id: Option<i64>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Launching,
    Running,
    Stopped,
    Terminated,
}

impl SessionState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Launching => "launching",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Terminated => "terminated",
        }
    }
}

pub struct DapShared {
    pub initialized: AtomicBool,
    pub dead: AtomicBool,
    pub state: Mutex<SessionState>,
    pub stopped: Mutex<StoppedState>,
    pub exit_code: Mutex<Option<i64>>,
    pub output_ring: Mutex<String>,
    pub stderr_tail: Mutex<String>,
    pub state_notify: Notify,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>,
}

type BoxWriter = Box<dyn AsyncWrite + Unpin + Send>;

pub struct DapClient {
    pub adapter: ResolvedAdapter,
    pub shared: Arc<DapShared>,
    writer: Arc<Mutex<BoxWriter>>,
    child: Mutex<Child>,
    next_seq: AtomicI64,
}

impl DapClient {
    /// Launch the DAP adapter process and connect its Content-Length message stream.
    pub async fn connect(adapter: ResolvedAdapter, cwd: &Path, startup_timeout: Duration) -> Result<Arc<Self>> {
        let mut cmd = Command::new(&adapter.binary);
        cmd.args(&adapter.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "spawn DAP adapter '{}' ({})",
                adapter.id,
                adapter.binary.display()
            )
        })?;

        let shared = Arc::new(DapShared {
            initialized: AtomicBool::new(false),
            dead: AtomicBool::new(false),
            state: Mutex::new(SessionState::Launching),
            stopped: Mutex::new(StoppedState::default()),
            exit_code: Mutex::new(None),
            output_ring: Mutex::new(String::new()),
            stderr_tail: Mutex::new(String::new()),
            state_notify: Notify::new(),
            pending: Mutex::new(HashMap::new()),
        });

        let (writer, reader): (BoxWriter, Box<dyn AsyncRead + Unpin + Send>) = if adapter.is_socket {
            let stdout = child.stdout.take().context("capture socket DAP stdout")?;
            let stderr = child.stderr.take().context("capture socket DAP stderr")?;
            let addr = discover_socket_addr(stdout, stderr, &shared, startup_timeout).await?;
            let stream = TcpStream::connect(&addr)
                .await
                .with_context(|| format!("connect to DAP adapter socket {addr}"))?;
            let (read_half, write_half) = stream.into_split();
            (Box::new(write_half), Box::new(read_half))
        } else {
            let stdin = child.stdin.take().context("capture DAP stdin")?;
            let stdout = child.stdout.take().context("capture DAP stdout")?;
            let stderr = child.stderr.take().context("capture DAP stderr")?;
            spawn_stderr_drain(stderr, Arc::clone(&shared));
            (Box::new(stdin), Box::new(stdout))
        };

        let writer = Arc::new(Mutex::new(writer));
        let client = Arc::new(Self {
            adapter,
            shared: Arc::clone(&shared),
            writer: Arc::clone(&writer),
            child: Mutex::new(child),
            next_seq: AtomicI64::new(1),
        });

        // Background DAP message reader
        {
            let shared = Arc::clone(&shared);
            let writer = Arc::clone(&writer);
            let adapter_id = client.adapter.id.clone();
            tokio::spawn(async move {
                let res = read_dap_loop(BufReader::new(reader), &shared, &writer).await;
                shared.dead.store(true, Ordering::SeqCst);
                *shared.state.lock().await = SessionState::Terminated;
                let reason = match res {
                    Ok(()) => format!("DAP adapter '{adapter_id}' exited"),
                    Err(e) => format!("DAP adapter '{adapter_id}' stopped: {e:#}"),
                };
                let tail = shared.stderr_tail.lock().await.trim().to_owned();
                let full = if tail.is_empty() {
                    reason
                } else {
                    format!("{reason}\n{tail}")
                };
                let mut pending = shared.pending.lock().await;
                for (_, tx) in pending.drain() {
                    let _ = tx.send(Err(anyhow!("{full}")));
                }
                shared.state_notify.notify_waiters();
            });
        }

        Ok(client)
    }

    pub async fn request(&self, command: &str, arguments: Value, req_timeout: Duration) -> Result<Value> {
        ensure!(
            !self.shared.dead.load(Ordering::SeqCst),
            "DAP adapter '{}' is no longer running",
            self.adapter.id
        );
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().await.insert(seq, tx);

        let msg = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        if let Err(err) = send_dap_frame(&self.writer, &msg).await {
            self.shared.pending.lock().await.remove(&seq);
            return Err(err);
        }

        match timeout(req_timeout, rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => bail!("DAP adapter closed while handling '{command}'"),
            Err(_) => {
                self.shared.pending.lock().await.remove(&seq);
                bail!("DAP request '{command}' timed out");
            }
        }
    }

    pub async fn wait_for_initialized(&self, wait_timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + wait_timeout;
        while !self.shared.initialized.load(Ordering::SeqCst) {
            if self.shared.dead.load(Ordering::SeqCst) {
                bail!("DAP adapter exited before sending initialized event");
            }
            let Some(rem) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
                bail!("timed out waiting for DAP initialized event");
            };
            let _ = timeout(rem.min(Duration::from_millis(100)), self.shared.state_notify.notified()).await;
        }
        Ok(())
    }

    /// Wait until debuggee enters `Stopped` or `Terminated`. Returns `true` if settled, `false` if timed out still running.
    pub async fn wait_for_stop(&self, wait_timeout: Duration) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + wait_timeout;
        loop {
            {
                let st = self.shared.state.lock().await;
                if matches!(*st, SessionState::Stopped | SessionState::Terminated) {
                    return Ok(true);
                }
            }
            if self.shared.dead.load(Ordering::SeqCst) {
                return Ok(true);
            }
            let Some(rem) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
                return Ok(false);
            };
            let _ = timeout(rem.min(Duration::from_millis(100)), self.shared.state_notify.notified()).await;
        }
    }

    pub async fn shutdown(&self) {
        if !self.shared.dead.load(Ordering::SeqCst) {
            let _ = self
                .request(
                    "disconnect",
                    json!({ "terminateDebuggee": true }),
                    Duration::from_secs(2),
                )
                .await;
        }
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = timeout(Duration::from_millis(400), child.wait()).await;
    }
}

async fn discover_socket_addr(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    shared: &Arc<DapShared>,
    startup_timeout: Duration,
) -> Result<String> {
    let re = Regex::new(r"(?:127\.0\.0\.1|localhost|\[::1\]):(\d{2,5})").expect("valid socket regex");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(4);

    let spawn_scanner = |stream: Box<dyn AsyncRead + Unpin + Send>,
                         tx: tokio::sync::mpsc::Sender<String>,
                         re: Regex,
                         shared: Arc<DapShared>| {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                if let Some(m) = re.find(&line) {
                    let _ = tx.send(m.as_str().to_owned()).await;
                }
                let mut tail = shared.stderr_tail.lock().await;
                tail.push_str(&line);
                if tail.len() > 2048 {
                    let cut = tail.floor_char_boundary(tail.len() - 2048);
                    tail.drain(..cut);
                }
                line.clear();
            }
        });
    };

    spawn_scanner(Box::new(stdout), tx.clone(), re.clone(), Arc::clone(shared));
    spawn_scanner(Box::new(stderr), tx, re, Arc::clone(shared));

    timeout(startup_timeout, rx.recv())
        .await
        .context("timed out waiting for DAP adapter socket address")?
        .context("DAP adapter exited without announcing a TCP port")
}

fn spawn_stderr_drain(stderr: tokio::process::ChildStderr, shared: Arc<DapShared>) {
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
                let cut = tail.floor_char_boundary(tail.len() - 2048);
                tail.drain(..cut);
            }
        }
    });
}

async fn send_dap_frame(writer: &Arc<Mutex<BoxWriter>>, msg: &Value) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    ensure!(body.len() <= MAX_DAP_BODY, "DAP message exceeds size limit");
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut w = writer.lock().await;
    w.write_all(header.as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

async fn read_dap_loop<R: AsyncRead + Unpin>(
    mut reader: BufReader<R>,
    shared: &Arc<DapShared>,
    writer: &Arc<Mutex<BoxWriter>>,
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
                content_length = Some(v.trim().parse::<usize>().context("parse DAP Content-Length")?);
            }
        }
        let Some(len) = content_length else {
            continue;
        };
        ensure!(len > 0 && len <= MAX_DAP_BODY, "invalid DAP body length {len}");
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await?;
        let msg: Value = serde_json::from_slice(&body).context("parse DAP JSON payload")?;
        handle_dap_message(msg, shared, writer).await?;
    }
}

async fn handle_dap_message(
    msg: Value,
    shared: &Arc<DapShared>,
    writer: &Arc<Mutex<BoxWriter>>,
) -> Result<()> {
    match msg.get("type").and_then(Value::as_str) {
        Some("response") => {
            if let Some(req_seq) = msg.get("request_seq").and_then(Value::as_i64)
                && let Some(tx) = shared.pending.lock().await.remove(&req_seq)
            {
                let success = msg.get("success").and_then(Value::as_bool).unwrap_or(false);
                if success {
                    let body = msg.get("body").cloned().unwrap_or_else(|| json!({}));
                    let _ = tx.send(Ok(body));
                } else {
                    let err_msg = msg
                        .pointer("/body/error/format")
                        .or_else(|| msg.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("DAP adapter request failed");
                    let _ = tx.send(Err(anyhow!("{err_msg}")));
                }
            }
        }
        Some("event") => {
            let event = msg.get("event").and_then(Value::as_str).unwrap_or_default();
            let body = msg.get("body").cloned().unwrap_or_else(|| json!({}));
            match event {
                "initialized" => {
                    shared.initialized.store(true, Ordering::SeqCst);
                    shared.state_notify.notify_waiters();
                }
                "stopped" => {
                    {
                        let mut st = shared.stopped.lock().await;
                        st.reason = body.get("reason").and_then(Value::as_str).map(str::to_owned);
                        st.thread_id = body.get("threadId").and_then(Value::as_i64);
                        st.description = body
                            .get("description")
                            .or_else(|| body.get("text"))
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                    *shared.state.lock().await = SessionState::Stopped;
                    shared.state_notify.notify_waiters();
                }
                "continued" => {
                    *shared.state.lock().await = SessionState::Running;
                    shared.state_notify.notify_waiters();
                }
                "exited" => {
                    *shared.exit_code.lock().await = body.get("exitCode").and_then(Value::as_i64);
                    *shared.state.lock().await = SessionState::Terminated;
                    shared.state_notify.notify_waiters();
                }
                "terminated" => {
                    *shared.state.lock().await = SessionState::Terminated;
                    shared.state_notify.notify_waiters();
                }
                "output" => {
                    if let Some(text) = body.get("output").and_then(Value::as_str) {
                        let cat = body.get("category").and_then(Value::as_str).unwrap_or("console");
                        if cat != "telemetry" {
                            let mut ring = shared.output_ring.lock().await;
                            ring.push_str(text);
                            if ring.len() > MAX_OUTPUT_RING {
                                let cut = ring.floor_char_boundary(ring.len() - MAX_OUTPUT_RING);
                                ring.drain(..cut);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Some("request") => {
            // Reject reverse requests such as runInTerminal cleanly.
            let seq = msg.get("seq").and_then(Value::as_i64).unwrap_or(0);
            let cmd = msg.get("command").and_then(Value::as_str).unwrap_or_default();
            let resp = json!({
                "seq": 0,
                "type": "response",
                "request_seq": seq,
                "command": cmd,
                "success": false,
                "message": "reverse requests are not supported in headless MCP debug sessions"
            });
            let _ = send_dap_frame(writer, &resp).await;
        }
        _ => {}
    }
    Ok(())
}
