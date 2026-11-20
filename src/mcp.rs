use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::{process::Command, time::timeout};

mod log_preview;
mod service_ready;

const INLINE_BYTES: usize = 12_000;
// The input limit of OMP's command-aware minimizer.
const MINIMIZER_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(50);

#[derive(Parser)]
#[command(
    name = "omp-codex-mcp",
    version,
    about = "Serve the local OMP editing and inspection tools over MCP"
)]
struct Cli {
    /// Directory containing omp-refactor, omp-shell and obash.
    #[arg(long)]
    bin_dir: PathBuf,
    /// OMP executable used for offline token counts.
    #[arg(long)]
    omp: PathBuf,
    /// Node executable for reading saved performance profiles.
    #[arg(long, requires = "profile_helper")]
    node: Option<PathBuf>,
    /// Path to the OMP profile summary adapter.
    #[arg(long, requires = "node")]
    profile_helper: Option<PathBuf>,
}

#[derive(Clone)]
struct OmpTools {
    bin_dir: PathBuf,
    omp: PathBuf,
    node: Option<PathBuf>,
    profile_helper: Option<PathBuf>,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadLines {
    /// Absolute project directory.
    root: PathBuf,
    /// Project-relative source path.
    file: PathBuf,
    /// First line, inclusive, numbered from one.
    start: usize,
    /// Last line, inclusive. Read up to 120 lines per call.
    end: usize,
}

#[derive(Deserialize, JsonSchema)]
struct HashlinePatch {
    /// Absolute project directory.
    root: PathBuf,
    /// Existing project-relative files exactly as listed in the patch.
    files: Vec<PathBuf>,
    /// OMP patch with current [path#tag] headers and PUT/CUT operations.
    patch: String,
}

#[derive(Deserialize, JsonSchema)]
struct AstRewrite {
    /// Absolute project directory.
    root: PathBuf,
    /// Existing project-relative files to scan.
    files: Vec<PathBuf>,
    language: String,
    pattern: String,
    replacement: String,
}

#[derive(Deserialize, JsonSchema)]
struct AstSearch {
    /// Absolute project directory.
    root: PathBuf,
    /// Existing project-relative files to scan.
    files: Vec<PathBuf>,
    language: String,
    pattern: String,
    /// Number of matches to skip across the listed files.
    offset: Option<usize>,
    /// Page size, 1 to 50. Default 20.
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct ParseShell {
    /// Bash source to parse.
    script: String,
}

#[derive(Deserialize, JsonSchema)]
struct LogSummary {
    /// Absolute directory containing the saved log.
    root: PathBuf,
    /// Log path relative to root.
    file: PathBuf,
    /// Exit code captured when the original command ran.
    exit_code: u8,
    /// Original command text, used to pick a filter.
    command: String,
}

#[derive(Deserialize, JsonSchema)]
struct CountFile {
    /// Absolute directory containing the file.
    root: PathBuf,
    /// File path relative to root.
    file: PathBuf,
}

#[tool_router]
impl OmpTools {
    #[tool(
        description = "Read source lines with an OMP Hashline path tag for anchored patches.",
        annotations(read_only_hint = true)
    )]
    async fn hashline_read(&self, Parameters(input): Parameters<ReadLines>) -> CallToolResult {
        let result = async {
            ensure!(
                input.start > 0 && input.end >= input.start && input.end - input.start < 120,
                "provide an inclusive range of 1 to 120 lines"
            );
            let mut command = self.refactor(&input.root, "read")?;
            command
                .arg("--start")
                .arg(input.start.to_string())
                .arg("--end")
                .arg(input.end.to_string())
                .arg("--")
                .arg(input.file);
            let output = checked(run(command).await?)?;
            preview_text(&output.stdout)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Preview an OMP Hashline edit as a full saved Git patch. Use current read tags.",
        annotations(read_only_hint = true)
    )]
    async fn hashline_preview(
        &self,
        Parameters(input): Parameters<HashlinePatch>,
    ) -> CallToolResult {
        let result = async {
            let mut command = self.refactor(&input.root, "hashline")?;
            let mut patch = tempfile::NamedTempFile::new().context("create Hashline input")?;
            patch.write_all(input.patch.as_bytes())?;
            command
                .arg("--patch")
                .arg(patch.path())
                .arg("--")
                .args(input.files);
            patch_result(checked(run(command).await?)?)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Preview a structural OMP AST rewrite on listed files as a full saved Git patch.",
        annotations(read_only_hint = true)
    )]
    async fn ast_preview(&self, Parameters(input): Parameters<AstRewrite>) -> CallToolResult {
        let result = async {
            let mut command = self.refactor(&input.root, "ast")?;
            command
                .arg(format!("--lang={}", input.language))
                .arg(format!("--pattern={}", input.pattern))
                .arg(format!("--rewrite={}", input.replacement))
                .arg("--")
                .args(input.files);
            patch_result(checked(run(command).await?)?)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Find syntax matches without writing files. Returns locations, captures and a pagination offset.",
        annotations(read_only_hint = true)
    )]
    async fn ast_find(&self, Parameters(input): Parameters<AstSearch>) -> CallToolResult {
        let result = async {
            let mut command = self.refactor(&input.root, "ast-find")?;
            command
                .arg(format!("--lang={}", input.language))
                .arg(format!("--pattern={}", input.pattern))
                .arg("--offset")
                .arg(input.offset.unwrap_or(0).to_string())
                .arg("--limit")
                .arg(input.limit.unwrap_or(20).to_string())
                .arg("--")
                .args(input.files);
            inspection_result(checked(capture(command, ".json").await?)?)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Read a code summary with original source line ranges. Use hashline_read for the exact lines before editing.",
        annotations(read_only_hint = true)
    )]
    async fn read_summary(&self, Parameters(input): Parameters<CountFile>) -> CallToolResult {
        let result = async {
            let mut command = self.refactor(&input.root, "read-summary")?;
            command.arg("--").arg(input.file);
            inspection_result(checked(capture(command, ".json").await?)?)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Parse Bash source to a syntax tree with obash without executing the script.",
        annotations(read_only_hint = true)
    )]
    async fn shell_parse(&self, Parameters(input): Parameters<ParseShell>) -> CallToolResult {
        let result = async {
            let mut command = Command::new(self.bin_dir.join("obash"));
            command
                .env_remove("BASH_ENV")
                .env_remove("ENV")
                .args(["--json=native", "--noprofile", "--norc", "-c"])
                .arg(input.script);
            let output = checked(run(command).await?)?;
            preview_text(&output.stdout)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Summarize a saved command log with OMP while retaining the original log and status.",
        annotations(read_only_hint = true)
    )]
    async fn log_minimize(&self, Parameters(input): Parameters<LogSummary>) -> CallToolResult {
        let result = async {
            let log = project_file(&input.root, &input.file)?;
            let original = File::open(&log).context("open original log")?;
            if input.exit_code != 0 || original.metadata()?.len() > MINIMIZER_INPUT_BYTES {
                let excerpt = log_preview::log_excerpt(original, INLINE_BYTES)?;
                return Ok(format!(
                    "exit {}; raw log: {}\n{excerpt}",
                    input.exit_code,
                    log.display(),
                ));
            }
            let mut command = Command::new(self.bin_dir.join("omp-shell"));
            command
                .arg("minimize")
                .arg(log)
                .arg(input.exit_code.to_string())
                .arg(input.command);
            let output = run(command).await?;
            let reported = preview_text(&output.stderr)?;
            ensure!(
                output.status.code() == Some(i32::from(input.exit_code))
                    && reported.starts_with(&format!("exit {};", input.exit_code)),
                "{reported}"
            );
            Ok(format!(
                "{reported}\n{}",
                log_preview::log_excerpt(output.stdout.reopen()?, INLINE_BYTES)?
            ))
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Count a local file with OMP's offline tokenizers.",
        annotations(read_only_hint = true)
    )]
    async fn token_count(&self, Parameters(input): Parameters<CountFile>) -> CallToolResult {
        let result = async {
            let path = project_file(&input.root, &input.file)?;
            let mut command = Command::new(&self.omp);
            command.arg("toks").arg(path).arg("--json");
            let output = checked(run(command).await?)?;
            preview_text(&output.stdout)
        }
        .await;
        tool_result(result)
    }

    #[tool(
        description = "Wait for a local service using loopback TCP and/or log text written after an explicit byte offset.",
        annotations(read_only_hint = true)
    )]
    async fn service_ready(
        &self,
        Parameters(input): Parameters<service_ready::ReadyRequest>,
    ) -> CallToolResult {
        tool_result(service_ready::wait(input).await)
    }

    #[tool(
        description = "Summarize a saved V8 .cpuprofile or macOS sample text report from the specified project.",
        annotations(read_only_hint = true)
    )]
    async fn profile_summary(&self, Parameters(input): Parameters<CountFile>) -> CallToolResult {
        let result = async {
            let path = project_file(&input.root, &input.file)?;
            let node = self
                .node
                .as_ref()
                .context("profile reader Node executable is not configured")?;
            let helper = self
                .profile_helper
                .as_ref()
                .context("profile helper is not configured")?;
            let mut command = Command::new(node);
            command
                .env_remove("NODE_OPTIONS")
                .arg("--disable-warning=ExperimentalWarning")
                .arg(helper)
                .arg(path);
            let output = checked(capture(command, ".txt").await?)?;
            preview_text(&output.stdout)
        }
        .await;
        tool_result(result)
    }
}

impl OmpTools {
    fn refactor(&self, root: &Path, subcommand: &str) -> Result<Command> {
        let root = project_root(root)?;
        let mut command = Command::new(self.bin_dir.join("omp-refactor"));
        command
            .current_dir(&root)
            .arg("--root")
            .arg(&root)
            .arg(subcommand);
        Ok(command)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OmpTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("omp-codex-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Use the saved full patch from preview tools. Inspect it and run git apply --check in the project before applying it.",
            )
    }
}

fn project_root(root: &Path) -> Result<PathBuf> {
    ensure!(root.is_absolute(), "root must be an absolute directory");
    let root = root.canonicalize().context("resolve project directory")?;
    ensure!(root.is_dir(), "root must be a directory");
    Ok(root)
}

fn project_file(root: &Path, file: &Path) -> Result<PathBuf> {
    let root = project_root(root)?;
    ensure!(
        file.components()
            .all(|part| matches!(part, Component::CurDir | Component::Normal(_))),
        "file must be project-relative"
    );
    let full = root
        .join(file)
        .canonicalize()
        .context("resolve project file")?;
    ensure!(
        full.starts_with(&root) && full.is_file(),
        "expected a file inside root"
    );
    Ok(full)
}

struct Captured {
    status: ExitStatus,
    stdout: NamedTempFile,
    stderr: NamedTempFile,
}

async fn run(command: Command) -> Result<Captured> {
    capture(command, ".patch").await
}

async fn capture(mut command: Command, suffix: &str) -> Result<Captured> {
    let stdout = tempfile::Builder::new()
        .prefix("omp-edit-")
        .suffix(suffix)
        .tempfile()?;
    let stderr = NamedTempFile::new()?;
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(stdout.reopen()?)
        .stderr(stderr.reopen()?);
    let status = timeout(COMMAND_TIMEOUT, command.status())
        .await
        .context("OMP tool timed out")?
        .context("run OMP tool")?;
    Ok(Captured {
        status,
        stdout,
        stderr,
    })
}

fn checked(output: Captured) -> Result<Captured> {
    ensure!(
        output.status.success(),
        "{}: {}",
        output.status,
        preview_text(&output.stderr)?
    );
    Ok(output)
}

fn patch_result(output: Captured) -> Result<String> {
    let summary = preview_text(&output.stderr)?;
    if output.stdout.as_file().metadata()?.len() == 0 {
        return Ok(format!("No patch generated.\n{summary}"));
    }
    let preview = preview_text(&output.stdout)?;
    let path = output.stdout.into_temp_path().keep()?;
    Ok(format!(
        "Full patch: {}\n{summary}\n{preview}",
        path.display()
    ))
}

fn inspection_result(output: Captured) -> Result<String> {
    if output.stdout.as_file().metadata()?.len() <= INLINE_BYTES as u64 {
        return preview_text(&output.stdout);
    }
    let path = output.stdout.into_temp_path().keep()?;
    Ok(format!(
        "Result exceeds the inline limit. Full JSON: {}",
        path.display()
    ))
}

fn preview_text(file: &NamedTempFile) -> Result<String> {
    let mut bytes = Vec::new();
    file.reopen()?
        .take((INLINE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() <= INLINE_BYTES {
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    let prefix = &bytes[..INLINE_BYTES];
    let end = prefix
        .iter()
        .rposition(|&byte| byte == b'\n')
        .unwrap_or(INLINE_BYTES);
    Ok(format!(
        "{}\n[Preview clipped]",
        String::from_utf8_lossy(&prefix[..end])
    ))
}

fn tool_result(result: Result<String>) -> CallToolResult {
    match result {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))]),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    ensure!(
        cli.bin_dir.is_absolute() && cli.omp.is_absolute(),
        "supply absolute executable paths"
    );
    ensure!(
        cli.node.as_ref().is_none_or(|path| path.is_absolute())
            && cli
                .profile_helper
                .as_ref()
                .is_none_or(|path| path.is_absolute()),
        "supply absolute profile reader paths"
    );
    let server = OmpTools {
        bin_dir: cli.bin_dir,
        omp: cli.omp,
        node: cli.node,
        profile_helper: cli.profile_helper,
        tool_router: OmpTools::tool_router(),
    }
    .serve(stdio())
    .await?;
    server.waiting().await?;
    Ok(())
}
