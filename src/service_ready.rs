use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use anyhow::{Context, Result, ensure};
use regex::bytes::{Regex, RegexBuilder};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::json;
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncSeekExt},
    net::TcpStream,
    time::{Instant, sleep, timeout},
};

const MAX_LOG_WINDOW: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ReadyRequest {
    /// Absolute project directory containing the task log, if supplied.
    root: PathBuf,
    /// Port to check by making a TCP connection on loopback.
    port: Option<u16>,
    /// Existing, append-only project-relative log for this task.
    log: Option<PathBuf>,
    /// Regex to match a complete new log line.
    log_pattern: Option<String>,
    /// Byte offset captured before launch. Use zero for a fresh log.
    log_offset: Option<u64>,
    /// Optional task process ID. A disappeared process reports exited.
    pid: Option<u32>,
    /// Wait in milliseconds, from 1 to 40000. Defaults to 10000.
    timeout_ms: Option<u64>,
}

pub(crate) async fn wait(input: ReadyRequest) -> Result<String> {
    ensure!(input.port != Some(0), "port must be nonzero");
    let timeout_ms = input.timeout_ms.unwrap_or(10_000);
    ensure!(
        (1..=40_000).contains(&timeout_ms),
        "timeout_ms must be 1 to 40000"
    );
    if let Some(pid) = input.pid {
        ensure!(
            (1..=i32::MAX as u32).contains(&pid),
            "pid must be a positive process ID"
        );
    }
    let mut log = match (input.log, input.log_pattern, input.log_offset) {
        (Some(path), Some(pattern), Some(offset)) => {
            ensure!(!pattern.is_empty(), "log_pattern must not be empty");
            let path = crate::project_file(&input.root, &path)?;
            Some(LogGate::open(path, &pattern, offset).await?)
        }
        (None, None, None) => None,
        _ => anyhow::bail!("provide log, log_pattern and log_offset together"),
    };
    ensure!(
        input.port.is_some() || log.is_some(),
        "provide port, log, or both"
    );

    let started = Instant::now();
    let mut log_ready = log.as_ref().map(|_| false);
    let mut port_ready = input.port.map(|_| false);
    let status = timeout(Duration::from_millis(timeout_ms), async {
        loop {
            if process_exited(input.pid)? {
                return Ok::<_, anyhow::Error>("exited");
            }
            if let Some(ref mut gate) = log {
                log_ready = Some(gate.poll().await?);
            }
            if let Some(port) = input.port {
                port_ready = Some(loopback_listening(port).await);
            }
            if log_ready.unwrap_or(true) && port_ready.unwrap_or(true) {
                return Ok(if process_exited(input.pid)? {
                    "exited"
                } else {
                    "ready"
                });
            }
            sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .unwrap_or(Ok("timed_out"))?;

    Ok(json!({
        "status": status,
        "pid": input.pid,
        "port": input.port,
        "port_ready": port_ready,
        "log_ready": log_ready,
        "elapsed_ms": started.elapsed().as_millis(),
    })
    .to_string())
}

struct LogGate {
    path: PathBuf,
    file: File,
    pattern: Regex,
    position: u64,
    line: Vec<u8>,
    skip_line: bool,
    matched: bool,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl LogGate {
    async fn open(path: PathBuf, pattern: &str, offset: u64) -> Result<Self> {
        let pattern = RegexBuilder::new(pattern)
            .size_limit(1024 * 1024)
            .build()
            .context("invalid readiness log regex")?;
        let mut file = File::open(&path).await.context("open readiness log")?;
        let metadata = file.metadata().await?;
        ensure!(
            offset <= metadata.len(),
            "log_offset is beyond the end of the log"
        );
        let mut skip_line = false;
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset - 1)).await?;
            skip_line = file.read_u8().await? != b'\n';
        }
        Ok(Self {
            path,
            file,
            pattern,
            position: offset,
            line: Vec::new(),
            skip_line,
            matched: false,
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    async fn poll(&mut self) -> Result<bool> {
        let metadata = fs::metadata(&self.path)
            .await
            .context("inspect readiness log")?;
        #[cfg(unix)]
        ensure!(
            (metadata.dev(), metadata.ino()) == self.identity,
            "readiness log was replaced; use the current task log and offset"
        );
        ensure!(
            metadata.len() >= self.position,
            "readiness log was truncated; use the current task log and offset"
        );
        if self.matched {
            return Ok(true);
        }
        // Keep partial lines across reads, without treating a chunk boundary
        // as the end of a log line.
        let mut remaining = (metadata.len() - self.position).min(MAX_LOG_WINDOW as u64);
        let mut chunk = [0; 8192];
        while remaining > 0 {
            let size = chunk.len().min(remaining as usize);
            let count = self.file.read(&mut chunk[..size]).await?;
            ensure!(count != 0, "readiness log changed while being read");
            remaining -= count as u64;
            self.position += count as u64;
            for piece in chunk[..count].split_inclusive(|&byte| byte == b'\n') {
                if !self.skip_line {
                    if self.line.len() + piece.len() <= MAX_LOG_WINDOW {
                        self.line.extend_from_slice(piece);
                    } else {
                        self.line.clear();
                        self.skip_line = true;
                    }
                }
                if piece.ends_with(b"\n") {
                    if !self.skip_line {
                        let content = &self.line[..self.line.len() - 1];
                        let content = content.strip_suffix(b"\r").unwrap_or(content);
                        if self.pattern.is_match(content) {
                            self.matched = true;
                            return Ok(true);
                        }
                    }
                    self.line.clear();
                    self.skip_line = false;
                }
            }
        }
        Ok(false)
    }
}

async fn loopback_listening(port: u16) -> bool {
    let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
    timeout(CONNECT_TIMEOUT, async {
        tokio::select! {
            Ok(_) = TcpStream::connect(v4) => true,
            Ok(_) = TcpStream::connect(v6) => true,
            else => false,
        }
    })
    .await
    .unwrap_or(false)
}

#[cfg(unix)]
fn process_exited(pid: Option<u32>) -> Result<bool> {
    let Some(pid) = pid else { return Ok(false) };
    // The validated PID is positive. Signal zero probes existence without
    // delivering a signal or changing process state.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(true),
        Some(libc::EPERM) => Ok(false),
        _ => Err(error).context("inspect readiness PID"),
    }
}

#[cfg(not(unix))]
fn process_exited(pid: Option<u32>) -> Result<bool> {
    ensure!(pid.is_none(), "pid readiness checks require a Unix host");
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[tokio::test]
    async fn log_ignores_prior_bytes_and_detects_a_split_marker() -> Result<()> {
        let mut original = tempfile::NamedTempFile::new()?;
        original.write_all(b"READY now\n")?;
        let offset = original.as_file().metadata()?.len();
        let mut gate = LogGate::open(original.path().to_owned(), "^READY now$", offset).await?;
        assert!(!gate.poll().await?);

        original.write_all(b"REA")?;
        assert!(!gate.poll().await?);
        original.write_all(b"DY now\n")?;
        assert!(gate.poll().await?);
        Ok(())
    }

    #[tokio::test]
    async fn anchored_matches_need_real_line_boundaries() -> Result<()> {
        let mut original = tempfile::NamedTempFile::new()?;
        original.write_all(b"old prefix ")?;
        let offset = original.as_file().metadata()?.len();
        let mut gate = LogGate::open(original.path().to_owned(), "^READY$", offset).await?;

        original.write_all(b"READY\n")?;
        assert!(!gate.poll().await?);
        original.write_all(b"READY")?;
        assert!(!gate.poll().await?);
        original.write_all(b" FAILED\n")?;
        assert!(!gate.poll().await?);
        original.write_all(b"READY\n")?;
        assert!(gate.poll().await?);
        Ok(())
    }

    #[tokio::test]
    async fn log_rejects_truncation_after_the_start_offset() -> Result<()> {
        let mut original = tempfile::NamedTempFile::new()?;
        original.write_all(b"old bytes\n")?;
        let offset = original.as_file().metadata()?.len();
        let mut gate = LogGate::open(original.path().to_owned(), "READY", offset).await?;

        original.as_file().set_len(0)?;
        let error = gate.poll().await.unwrap_err();
        assert!(error.to_string().contains("truncated"));
        Ok(())
    }
}
