//! Zero-pollution one-click adapter for mainstream international & Chinese AI coding clients:
//! Antigravity (native plugin or global MCP), Claude Code, Codex, Cursor,
//! VS Code / GitHub Copilot, Claude Desktop, Windsurf, Zed,
//! WorkBuddy, CodeBuddy, Trae / Trae CN, Qoder, Tongyi Lingma, Kiro, Cline, and Roo Code.
//!
//! Design guarantees:
//! - Never edits global instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`).
//! - Never touches Antigravity's internal `config.json`.
//! - Preserves all unrelated MCP servers and settings fields via atomic JSON updates.
//! - Refuses to overwrite an existing `omp` entry pointing elsewhere unless `--force` is set.
//! - Captures a sanitized `PATH` into `env.PATH` for GUI apps so warm LSP/DAP servers
//!   find tools like `rust-analyzer`, `gopls`, `pyright`, `lldb-dap`, and `dlv`.

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::diff_util::write_atomic;

const SERVER_KEY: &str = "omp";
const PLUGIN_DIR_NAME: &str = "omp-toolkit";

/// Supported AI coding software targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Client {
    /// Google Antigravity native plugin (`~/.gemini/config/plugins/omp-toolkit/`).
    Antigravity,
    /// Google Antigravity global MCP config (`~/.gemini/config/mcp_config.json`).
    AntigravityMcp,
    /// Anthropic Claude Code CLI (`claude mcp add/remove`).
    ClaudeCode,
    /// OpenAI ChatGPT Desktop App & Codex CLI (`~/.codex/config.toml` -> `[mcp_servers.omp]`).
    Codex,
    /// Cursor (`~/.cursor/mcp.json`).
    Cursor,
    /// VS Code / GitHub Copilot user `mcp.json`.
    VsCode,
    /// Claude Desktop (`claude_desktop_config.json`).
    ClaudeDesktop,
    /// Windsurf (`~/.codeium/windsurf/mcp_config.json`).
    Windsurf,
    /// Zed Editor (prints `context_servers` JSON snippet; does not rewrite JSONC).
    Zed,
    /// Tencent WorkBuddy desktop (`~/.workbuddy-ai/mcp.json` or shared `~/.codebuddy/.mcp.json`).
    WorkBuddy,
    /// Tencent CodeBuddy CLI / IDE (`~/.codebuddy/.mcp.json`).
    CodeBuddy,
    /// ByteDance Trae / Trae CN (`~/.trae/mcp.json`).
    Trae,
    /// Alibaba Cloud Qoder (`~/.qoder/settings.json` -> `mcpServers.omp`).
    Qoder,
    /// Alibaba Tongyi Lingma IDE plugin (prints standard `mcpServers` snippet for UI import).
    Lingma,
    /// AWS Kiro (`~/.kiro/settings/mcp.json`).
    Kiro,
    /// Cline VS Code extension (`cline_mcp_settings.json`).
    Cline,
    /// Roo Code VS Code extension (`mcp_settings.json`).
    RooCode,
}

impl Client {
    pub const ALL: &[Self] = &[
        Self::Antigravity,
        Self::AntigravityMcp,
        Self::ClaudeCode,
        Self::Codex,
        Self::Cursor,
        Self::VsCode,
        Self::ClaudeDesktop,
        Self::Windsurf,
        Self::Zed,
        Self::WorkBuddy,
        Self::CodeBuddy,
        Self::Trae,
        Self::Qoder,
        Self::Lingma,
        Self::Kiro,
        Self::Cline,
        Self::RooCode,
    ];

    /// Clients evaluated during `--client auto` detection.
    pub const AUTO_CANDIDATES: &[Self] = &[
        Self::Antigravity,
        Self::ClaudeCode,
        Self::Codex,
        Self::Cursor,
        Self::VsCode,
        Self::ClaudeDesktop,
        Self::Windsurf,
        Self::WorkBuddy,
        Self::CodeBuddy,
        Self::Trae,
        Self::Qoder,
        Self::Kiro,
        Self::Cline,
        Self::RooCode,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::AntigravityMcp => "antigravity-mcp",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::VsCode => "vscode",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Windsurf => "windsurf",
            Self::Zed => "zed",
            Self::WorkBuddy => "workbuddy",
            Self::CodeBuddy => "codebuddy",
            Self::Trae => "trae",
            Self::Qoder => "qoder",
            Self::Lingma => "lingma",
            Self::Kiro => "kiro",
            Self::Cline => "cline",
            Self::RooCode => "roo-code",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity (Native Plugin)",
            Self::AntigravityMcp => "Antigravity (Global MCP)",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "ChatGPT Desktop / Codex",
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code / GitHub Copilot",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Windsurf => "Windsurf",
            Self::Zed => "Zed",
            Self::WorkBuddy => "WorkBuddy (Tencent)",
            Self::CodeBuddy => "CodeBuddy (Tencent)",
            Self::Trae => "Trae / Trae CN (ByteDance)",
            Self::Qoder => "Qoder (Alibaba Cloud)",
            Self::Lingma => "Tongyi Lingma (Alibaba)",
            Self::Kiro => "Kiro (AWS)",
            Self::Cline => "Cline (VS Code)",
            Self::RooCode => "Roo Code (VS Code)",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "antigravity" | "agy" | "antigravity-plugin" => Ok(Self::Antigravity),
            "antigravity-mcp" | "agy-mcp" => Ok(Self::AntigravityMcp),
            "claude" | "claude-code" => Ok(Self::ClaudeCode),
            "codex" | "chatgpt" | "chatgpt-desktop" | "codex-app" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "vscode" | "vs-code" | "copilot" => Ok(Self::VsCode),
            "claude-desktop" => Ok(Self::ClaudeDesktop),
            "windsurf" => Ok(Self::Windsurf),
            "zed" => Ok(Self::Zed),
            "workbuddy" | "work-buddy" => Ok(Self::WorkBuddy),
            "codebuddy" | "code-buddy" => Ok(Self::CodeBuddy),
            "trae" | "trae-cn" => Ok(Self::Trae),
            "qoder" | "qoder-cn" => Ok(Self::Qoder),
            "lingma" | "tongyi-lingma" | "tongyi" => Ok(Self::Lingma),
            "kiro" | "kiro-cli" => Ok(Self::Kiro),
            "cline" => Ok(Self::Cline),
            "roo-code" | "roocode" | "roo" => Ok(Self::RooCode),
            other => bail!(
                "unknown client '{other}'; supported: auto, antigravity, antigravity-mcp, claude-code, codex, cursor, vscode, claude-desktop, windsurf, zed, workbuddy, codebuddy, trae, qoder, lingma, kiro, cline, roo-code"
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SetupOptions {
    /// Absolute path to the `omp-toolkit` executable to register.
    pub binary: PathBuf,
    /// Optional explicit HOME override (primarily for tests or custom profiles).
    pub home: Option<PathBuf>,
    /// Optional explicit `env.PATH` embedded into GUI MCP entries (defaults to current sanitized PATH).
    pub path_env: Option<String>,
    /// Preview all actions without modifying any files or invoking external CLIs.
    pub dry_run: bool,
    /// Overwrite an existing `omp` entry even if it points to another binary.
    pub force: bool,
    /// Include `env.PATH` in generated MCP configs (recommended for GUI apps on macOS/Linux).
    pub include_path_env: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientDetection {
    pub client: String,
    pub display_name: String,
    pub detected: bool,
    pub target: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionReport {
    pub client: String,
    pub display_name: String,
    pub action: String,
    pub status: String,
    pub target: String,
    pub message: String,
}

/// Detect which supported AI coding clients are present on this machine.
pub fn detect_clients(home_override: Option<&Path>) -> Result<Vec<ClientDetection>> {
    let check_system_apps = home_override.is_none();
    let home = resolve_home(home_override)?;
    let mut list = Vec::with_capacity(Client::ALL.len());
    for &client in Client::ALL {
        let (detected, target, notes) = inspect_client(client, &home, check_system_apps);
        list.push(ClientDetection {
            client: client.id().to_owned(),
            display_name: client.display_name().to_owned(),
            detected,
            target,
            notes,
        });
    }
    Ok(list)
}

/// Print the ready-to-paste configuration snippet or CLI command for `client` without modifying disk.
pub fn render_snippet(client: Client, opts: &SetupOptions) -> Result<String> {
    let bin = opts.binary.to_string_lossy();
    let path_env = resolved_path_env(opts);
    let mcp_entry = build_mcp_entry(&opts.binary, path_env.clone());
    let typed_entry = build_typed_stdio_mcp_entry(&opts.binary, path_env);
    let snippet = match client {
        Client::Antigravity => {
            let plugin = build_antigravity_plugin_manifest();
            let mcp = json!({ "mcpServers": { SERVER_KEY: mcp_entry } });
            format!(
                "# ~/.gemini/config/plugins/{PLUGIN_DIR_NAME}/plugin.json\n{}\n\n# ~/.gemini/config/plugins/{PLUGIN_DIR_NAME}/mcp_config.json\n{}\n",
                serde_json::to_string_pretty(&plugin)?,
                serde_json::to_string_pretty(&mcp)?
            )
        }
        Client::AntigravityMcp
        | Client::Cursor
        | Client::ClaudeDesktop
        | Client::Windsurf
        | Client::Trae
        | Client::Lingma
        | Client::Kiro
        | Client::Cline
        | Client::RooCode => {
            serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER_KEY: mcp_entry } }))?
        }
        Client::WorkBuddy | Client::CodeBuddy | Client::Qoder => {
            serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER_KEY: typed_entry } }))?
        }
        Client::VsCode => {
            serde_json::to_string_pretty(&json!({ "servers": { SERVER_KEY: typed_entry } }))?
        }
        Client::Zed => {
            let mut zed_entry = json!({
                "source": "custom",
                "command": bin,
                "args": ["mcp"],
            });
            if let Some(path_val) = resolved_path_env(opts) {
                zed_entry["env"] = json!({ "PATH": path_val });
            }
            serde_json::to_string_pretty(&json!({ "context_servers": { SERVER_KEY: zed_entry } }))?
        }
        Client::ClaudeCode => {
            format!(
                "claude mcp add --scope user --transport stdio {SERVER_KEY} -- \"{bin}\" mcp"
            )
        }
        Client::Codex => {
            let block = render_codex_toml_block(&opts.binary, resolved_path_env(opts));
            format!(
                "# ~/.codex/config.toml (shared by ChatGPT Desktop & Codex CLI)\n{block}\n# Or via Codex CLI:\n# codex mcp add {SERVER_KEY} -- \"{bin}\" mcp"
            )
        }
    };
    Ok(snippet)
}

/// Configure one or more clients (or `"auto"` for all detected clients).
pub fn apply_clients(clients: &[Client], opts: &SetupOptions) -> Result<Vec<ActionReport>> {
    ensure!(
        opts.binary.is_absolute(),
        "binary path must be absolute: {}",
        opts.binary.display()
    );
    let home = resolve_home(opts.home.as_deref())?;
    let mut reports = Vec::with_capacity(clients.len());
    for &client in clients {
        reports.push(setup_one(client, &home, opts)?);
    }
    Ok(reports)
}

/// Resolve `"auto"` into the list of detected clients on this machine.
pub fn resolve_auto_clients(home_override: Option<&Path>) -> Result<Vec<Client>> {
    let check_system_apps = home_override.is_none();
    let home = resolve_home(home_override)?;
    let mut found = Vec::new();
    for &client in Client::AUTO_CANDIDATES {
        let (detected, _, _) = inspect_client(client, &home, check_system_apps);
        if detected {
            found.push(client);
        }
    }
    Ok(found)
}

/// Remove `omp-toolkit` integration cleanly from the selected clients.
pub fn remove_clients(clients: &[Client], opts: &SetupOptions) -> Result<Vec<ActionReport>> {
    let home = resolve_home(opts.home.as_deref())?;
    let mut reports = Vec::with_capacity(clients.len());
    for &client in clients {
        reports.push(remove_one(client, &home, opts)?);
    }
    Ok(reports)
}

/// Atomically install the current executable into `dest_dir/omp-toolkit[.exe]`.
pub fn install_binary(source_bin: &Path, dest_dir: &Path, dry_run: bool) -> Result<PathBuf> {
    let file_name = format!("omp-toolkit{}", env::consts::EXE_SUFFIX);
    let dest = dest_dir.join(file_name);
    if dry_run {
        return Ok(dest);
    }
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("create install directory {}", dest_dir.display()))?;
    let canonical_src = source_bin.canonicalize().unwrap_or_else(|_| source_bin.to_path_buf());
    let canonical_dest_dir = dest_dir.canonicalize().unwrap_or_else(|_| dest_dir.to_path_buf());
    let canonical_dest = canonical_dest_dir.join(format!("omp-toolkit{}", env::consts::EXE_SUFFIX));
    if canonical_src == canonical_dest {
        return Ok(canonical_dest);
    }
    let bytes = fs::read(&canonical_src)
        .with_context(|| format!("read binary {}", canonical_src.display()))?;
    write_atomic(&canonical_dest, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&canonical_dest, fs::Permissions::from_mode(0o755))?;
    }
    Ok(canonical_dest)
}

fn setup_one(client: Client, home: &Path, opts: &SetupOptions) -> Result<ActionReport> {
    let path_env = resolved_path_env(opts);
    let use_env = opts.home.is_none();
    match client {
        Client::Antigravity => setup_antigravity_plugin(home, opts, path_env),
        Client::AntigravityMcp => {
            let path = antigravity_mcp_path(home);
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::Cursor => {
            let path = home.join(".cursor").join("mcp.json");
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::VsCode => {
            let path = vscode_mcp_path(home, use_env);
            let entry = build_typed_stdio_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "servers", entry, opts)
        }
        Client::ClaudeDesktop => {
            let path = claude_desktop_config_path(home, use_env);
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::Windsurf => {
            let path = home.join(".codeium").join("windsurf").join("mcp_config.json");
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::WorkBuddy => {
            let path = workbuddy_mcp_path(home);
            let entry = build_typed_stdio_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::CodeBuddy => {
            let path = codebuddy_mcp_path(home);
            let entry = build_typed_stdio_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::Trae => {
            let path = trae_mcp_path(home);
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::Qoder => {
            let path = qoder_settings_path(home);
            let entry = build_typed_stdio_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::Kiro => {
            let path = kiro_mcp_path(home);
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::Cline => {
            let path = cline_mcp_path(home, use_env);
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::RooCode => {
            let path = roo_code_mcp_path(home, use_env);
            let entry = build_mcp_entry(&opts.binary, path_env);
            merge_json_mcp(client, &path, "mcpServers", entry, opts)
        }
        Client::ClaudeCode => setup_cli_client(client, "claude", opts),
        Client::Codex => setup_codex_toml(home, opts, path_env),
        Client::Zed => Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "manual".into(),
            status: "skipped".into(),
            target: zed_settings_path(home).display().to_string(),
            message: "Zed settings.json uses JSONC comments; run `omp-toolkit setup --print zed` and paste under `context_servers`.".into(),
        }),
        Client::Lingma => Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "manual".into(),
            status: "skipped".into(),
            target: home.join(".lingma").display().to_string(),
            message: "Tongyi Lingma manages MCP via IDE sidebar (Personal Settings -> MCP -> Manual JSON); run `omp-toolkit setup --print lingma` and paste it there.".into(),
        }),
    }
}

fn remove_one(client: Client, home: &Path, opts: &SetupOptions) -> Result<ActionReport> {
    let use_env = opts.home.is_none();
    match client {
        Client::Antigravity => {
            let dir = antigravity_plugin_dir(home);
            if !dir.exists() {
                return Ok(ActionReport {
                    client: client.id().into(),
                    display_name: client.display_name().into(),
                    action: "remove".into(),
                    status: "unchanged".into(),
                    target: dir.display().to_string(),
                    message: "Antigravity plugin directory does not exist.".into(),
                });
            }
            if opts.dry_run {
                return Ok(ActionReport {
                    client: client.id().into(),
                    display_name: client.display_name().into(),
                    action: "remove".into(),
                    status: "dry_run".into(),
                    target: dir.display().to_string(),
                    message: format!("Would remove plugin directory {}", dir.display()),
                });
            }
            fs::remove_dir_all(&dir)
                .with_context(|| format!("remove plugin directory {}", dir.display()))?;
            Ok(ActionReport {
                client: client.id().into(),
                display_name: client.display_name().into(),
                action: "remove".into(),
                status: "removed".into(),
                target: dir.display().to_string(),
                message: "Removed Antigravity plugin bundle.".into(),
            })
        }
        Client::AntigravityMcp => {
            remove_json_mcp(client, &antigravity_mcp_path(home), "mcpServers", opts)
        }
        Client::Cursor => {
            remove_json_mcp(client, &home.join(".cursor").join("mcp.json"), "mcpServers", opts)
        }
        Client::VsCode => remove_json_mcp(client, &vscode_mcp_path(home, use_env), "servers", opts),
        Client::ClaudeDesktop => {
            remove_json_mcp(client, &claude_desktop_config_path(home, use_env), "mcpServers", opts)
        }
        Client::Windsurf => remove_json_mcp(
            client,
            &home.join(".codeium").join("windsurf").join("mcp_config.json"),
            "mcpServers",
            opts,
        ),
        Client::WorkBuddy => {
            remove_json_mcp(client, &workbuddy_mcp_path(home), "mcpServers", opts)
        }
        Client::CodeBuddy => {
            remove_json_mcp(client, &codebuddy_mcp_path(home), "mcpServers", opts)
        }
        Client::Trae => remove_json_mcp(client, &trae_mcp_path(home), "mcpServers", opts),
        Client::Qoder => remove_json_mcp(client, &qoder_settings_path(home), "mcpServers", opts),
        Client::Kiro => remove_json_mcp(client, &kiro_mcp_path(home), "mcpServers", opts),
        Client::Cline => remove_json_mcp(client, &cline_mcp_path(home, use_env), "mcpServers", opts),
        Client::RooCode => {
            remove_json_mcp(client, &roo_code_mcp_path(home, use_env), "mcpServers", opts)
        }
        Client::ClaudeCode => remove_cli_client(client, "claude", opts),
        Client::Codex => remove_codex_toml(home, opts),
        Client::Zed => Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "skipped".into(),
            target: zed_settings_path(home).display().to_string(),
            message: "Remove the `omp` entry manually from Zed settings.json `context_servers`.".into(),
        }),
        Client::Lingma => Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "skipped".into(),
            target: home.join(".lingma").display().to_string(),
            message: "Remove `omp` directly from Tongyi Lingma IDE MCP settings panel.".into(),
        }),
    }
}

fn setup_antigravity_plugin(
    home: &Path,
    opts: &SetupOptions,
    path_env: Option<String>,
) -> Result<ActionReport> {
    let dir = antigravity_plugin_dir(home);
    let manifest_path = dir.join("plugin.json");
    let mcp_path = dir.join("mcp_config.json");
    let skill_path = dir
        .join("skills")
        .join("omp-code-intelligence")
        .join("SKILL.md");

    let manifest = serde_json::to_string_pretty(&build_antigravity_plugin_manifest())? + "\n";
    let mcp_entry = build_mcp_entry(&opts.binary, path_env);
    let mcp_json = serde_json::to_string_pretty(&json!({
        "mcpServers": { SERVER_KEY: mcp_entry }
    }))? + "\n";
    let skill_md = antigravity_skill_markdown();

    if opts.dry_run {
        return Ok(ActionReport {
            client: Client::Antigravity.id().into(),
            display_name: Client::Antigravity.display_name().into(),
            action: "setup".into(),
            status: "dry_run".into(),
            target: dir.display().to_string(),
            message: format!(
                "Would write plugin.json, mcp_config.json, and skills/omp-code-intelligence/SKILL.md in {}",
                dir.display()
            ),
        });
    }

    fs::create_dir_all(skill_path.parent().unwrap())?;
    write_atomic(&manifest_path, manifest.as_bytes())?;
    write_atomic(&mcp_path, mcp_json.as_bytes())?;
    write_atomic(&skill_path, skill_md.as_bytes())?;

    Ok(ActionReport {
        client: Client::Antigravity.id().into(),
        display_name: Client::Antigravity.display_name().into(),
        action: "setup".into(),
        status: "configured".into(),
        target: dir.display().to_string(),
        message: "Installed native Antigravity plugin (toggle anytime in Customizations without restarting).".into(),
    })
}

fn merge_json_mcp(
    client: Client,
    config_path: &Path,
    container_key: &str,
    entry: Value,
    opts: &SetupOptions,
) -> Result<ActionReport> {
    let mut root_obj = if config_path.is_file() {
        let raw = fs::read_to_string(config_path)
            .with_context(|| format!("read {}", config_path.display()))?;
        if raw.trim().is_empty() {
            Map::new()
        } else {
            let parsed: Value = serde_json::from_str(&raw).with_context(|| {
                format!(
                    "parse {} as JSON (refusing to overwrite malformed or JSONC file)",
                    config_path.display()
                )
            })?;
            parsed.as_object().cloned().with_context(|| {
                format!("expected top-level JSON object in {}", config_path.display())
            })?
        }
    } else {
        Map::new()
    };

    let servers = root_obj
        .entry(container_key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .with_context(|| format!("`{container_key}` in {} is not a JSON object", config_path.display()))?;

    if let Some(existing) = servers.get(SERVER_KEY) {
        if existing == &entry {
            return Ok(ActionReport {
                client: client.id().into(),
                display_name: client.display_name().into(),
                action: "setup".into(),
                status: "unchanged".into(),
                target: config_path.display().to_string(),
                message: "Existing `omp` entry already matches this binary.".into(),
            });
        }
        let points_to_omp = existing
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("omp-toolkit"));
        if !opts.force && !points_to_omp {
            return Ok(ActionReport {
                client: client.id().into(),
                display_name: client.display_name().into(),
                action: "setup".into(),
                status: "preserved".into(),
                target: config_path.display().to_string(),
                message: "Existing `omp` entry points to another command; preserved (pass --force to replace).".into(),
            });
        }
    }

    if opts.dry_run {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "setup".into(),
            status: "dry_run".into(),
            target: config_path.display().to_string(),
            message: format!(
                "Would set `{container_key}.{SERVER_KEY}` in {}",
                config_path.display()
            ),
        });
    }

    servers.insert(SERVER_KEY.to_owned(), entry);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let formatted = serde_json::to_string_pretty(&Value::Object(root_obj))? + "\n";
    write_atomic(config_path, formatted.as_bytes())?;

    Ok(ActionReport {
        client: client.id().into(),
        display_name: client.display_name().into(),
        action: "setup".into(),
        status: "configured".into(),
        target: config_path.display().to_string(),
        message: format!("Configured `{container_key}.{SERVER_KEY}` atomically."),
    })
}

fn remove_json_mcp(
    client: Client,
    config_path: &Path,
    container_key: &str,
    opts: &SetupOptions,
) -> Result<ActionReport> {
    if !config_path.is_file() {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "unchanged".into(),
            target: config_path.display().to_string(),
            message: "Configuration file does not exist.".into(),
        });
    }
    let raw = fs::read_to_string(config_path)?;
    let mut parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", config_path.display()))?;
    let removed = parsed
        .as_object_mut()
        .and_then(|o| o.get_mut(container_key))
        .and_then(Value::as_object_mut)
        .is_some_and(|servers| servers.remove(SERVER_KEY).is_some());

    if !removed {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "unchanged".into(),
            target: config_path.display().to_string(),
            message: "No `omp` entry found.".into(),
        });
    }

    if opts.dry_run {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "dry_run".into(),
            target: config_path.display().to_string(),
            message: format!("Would remove `{container_key}.{SERVER_KEY}` from {}", config_path.display()),
        });
    }

    let formatted = serde_json::to_string_pretty(&parsed)? + "\n";
    write_atomic(config_path, formatted.as_bytes())?;
    Ok(ActionReport {
        client: client.id().into(),
        display_name: client.display_name().into(),
        action: "remove".into(),
        status: "removed".into(),
        target: config_path.display().to_string(),
        message: format!("Removed `{container_key}.{SERVER_KEY}`."),
    })
}

fn setup_codex_toml(
    home: &Path,
    opts: &SetupOptions,
    path_env: Option<String>,
) -> Result<ActionReport> {
    let config_path = codex_config_path(home);
    let desired_block = render_codex_toml_block(&opts.binary, path_env);

    let existing_raw = if config_path.is_file() {
        fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?
    } else {
        String::new()
    };

    if !existing_raw.trim().is_empty() {
        let parsed: toml::Value = toml::from_str(&existing_raw).with_context(|| {
            format!(
                "parse {} as TOML (refusing to overwrite malformed config.toml)",
                config_path.display()
            )
        })?;
        if let Some(existing_cmd) = parsed
            .get("mcp_servers")
            .and_then(|t| t.get(SERVER_KEY))
            .and_then(|s| s.get("command"))
            .and_then(toml::Value::as_str)
        {
            let points_to_omp = existing_cmd.contains("omp-toolkit");
            if !opts.force && !points_to_omp {
                return Ok(ActionReport {
                    client: Client::Codex.id().into(),
                    display_name: Client::Codex.display_name().into(),
                    action: "setup".into(),
                    status: "preserved".into(),
                    target: config_path.display().to_string(),
                    message: "Existing `[mcp_servers.omp]` points to another command; preserved (pass --force to replace).".into(),
                });
            }
        }
    }

    let updated = upsert_codex_toml_section(&existing_raw, &desired_block);
    // Validate that the final TOML parses cleanly before touching disk.
    toml::from_str::<toml::Value>(&updated).with_context(|| {
        format!(
            "internal validation failed for updated {}",
            config_path.display()
        )
    })?;

    if updated == existing_raw {
        return Ok(ActionReport {
            client: Client::Codex.id().into(),
            display_name: Client::Codex.display_name().into(),
            action: "setup".into(),
            status: "unchanged".into(),
            target: config_path.display().to_string(),
            message: "Existing `[mcp_servers.omp]` already matches this binary.".into(),
        });
    }

    if opts.dry_run {
        return Ok(ActionReport {
            client: Client::Codex.id().into(),
            display_name: Client::Codex.display_name().into(),
            action: "setup".into(),
            status: "dry_run".into(),
            target: config_path.display().to_string(),
            message: format!(
                "Would update `[mcp_servers.{SERVER_KEY}]` in {} (preserving comments and all other settings).",
                config_path.display()
            ),
        });
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&config_path, updated.as_bytes())?;

    Ok(ActionReport {
        client: Client::Codex.id().into(),
        display_name: Client::Codex.display_name().into(),
        action: "setup".into(),
        status: "configured".into(),
        target: config_path.display().to_string(),
        message: "Updated `[mcp_servers.omp]` in `~/.codex/config.toml` atomically (restart ChatGPT Desktop / Codex to reload; AGENTS.md untouched).".into(),
    })
}

fn remove_codex_toml(home: &Path, opts: &SetupOptions) -> Result<ActionReport> {
    let config_path = codex_config_path(home);
    if !config_path.is_file() {
        return Ok(ActionReport {
            client: Client::Codex.id().into(),
            display_name: Client::Codex.display_name().into(),
            action: "remove".into(),
            status: "unchanged".into(),
            target: config_path.display().to_string(),
            message: "Codex config.toml does not exist.".into(),
        });
    }
    let existing_raw = fs::read_to_string(&config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    let (updated, removed) = strip_codex_omp_sections(&existing_raw);
    if !removed {
        return Ok(ActionReport {
            client: Client::Codex.id().into(),
            display_name: Client::Codex.display_name().into(),
            action: "remove".into(),
            status: "unchanged".into(),
            target: config_path.display().to_string(),
            message: "No `[mcp_servers.omp]` entry found.".into(),
        });
    }
    toml::from_str::<toml::Value>(&updated).with_context(|| {
        format!(
            "internal validation failed after removing omp from {}",
            config_path.display()
        )
    })?;

    if opts.dry_run {
        return Ok(ActionReport {
            client: Client::Codex.id().into(),
            display_name: Client::Codex.display_name().into(),
            action: "remove".into(),
            status: "dry_run".into(),
            target: config_path.display().to_string(),
            message: format!(
                "Would remove `[mcp_servers.{SERVER_KEY}]` from {}",
                config_path.display()
            ),
        });
    }

    write_atomic(&config_path, updated.as_bytes())?;
    Ok(ActionReport {
        client: Client::Codex.id().into(),
        display_name: Client::Codex.display_name().into(),
        action: "remove".into(),
        status: "removed".into(),
        target: config_path.display().to_string(),
        message: "Removed `[mcp_servers.omp]` from Codex config.toml.".into(),
    })
}

fn render_codex_toml_block(binary: &Path, path_env: Option<String>) -> String {
    let cmd_lit = toml::Value::String(binary.to_string_lossy().into_owned()).to_string();
    let mut out = format!("[mcp_servers.{SERVER_KEY}]\ncommand = {cmd_lit}\nargs = [\"mcp\"]\n");
    if let Some(path_val) = path_env {
        let path_lit = toml::Value::String(path_val).to_string();
        out.push_str(&format!("\n[mcp_servers.{SERVER_KEY}.env]\nPATH = {path_lit}\n"));
    }
    out
}

fn is_codex_omp_header(trimmed: &str) -> bool {
    let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    let name = inner.trim();
    name == format!("mcp_servers.{SERVER_KEY}")
        || name == format!("mcp_servers.\"{SERVER_KEY}\"")
        || name.starts_with(&format!("mcp_servers.{SERVER_KEY}."))
        || name.starts_with(&format!("mcp_servers.\"{SERVER_KEY}\"."))
}

fn is_toml_table_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && !trimmed.starts_with("[[") && trimmed.ends_with(']')
        || trimmed.starts_with("[[") && trimmed.ends_with("]]")
}

/// Remove `[mcp_servers.omp]` and any `[mcp_servers.omp.*]` sub-tables while preserving
/// all comments, formatting, and sibling tables byte-for-byte.
fn strip_codex_omp_sections(raw: &str) -> (String, bool) {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut kept: Vec<String> = Vec::new();
    let mut skipping = false;
    let mut removed_any = false;

    for raw_line in raw.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed = line.trim();
        if is_toml_table_header(trimmed) {
            if is_codex_omp_header(trimmed) {
                skipping = true;
                removed_any = true;
                continue;
            }
            skipping = false;
        }
        if !skipping {
            kept.push(line.to_owned());
        }
    }

    if !removed_any {
        return (raw.to_owned(), false);
    }

    // Collapse trailing excess blank lines introduced by deletion.
    while kept.len() >= 2 && kept[kept.len() - 1].is_empty() && kept[kept.len() - 2].is_empty() {
        kept.pop();
    }
    let mut out = kept.join(eol);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    (out, true)
}

/// Upsert `[mcp_servers.omp]` in-place (preserving position near existing `mcp_servers` if present)
/// while keeping every comment and unrelated table untouched.
fn upsert_codex_toml_section(raw: &str, block: &str) -> String {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let normalized_block = block.trim_end_matches('\n').replace('\n', eol);
    let lines: Vec<&str> = if raw.is_empty() {
        Vec::new()
    } else {
        raw.split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect()
    };

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 8);
    let mut skipping = false;
    let mut inserted = false;

    for line in &lines {
        let trimmed = line.trim();
        if is_toml_table_header(trimmed) {
            if is_codex_omp_header(trimmed) {
                if !inserted {
                    // Replace at the original `[mcp_servers.omp]` slot.
                    for blk_line in normalized_block.split(eol) {
                        out.push(blk_line.to_owned());
                    }
                    out.push(String::new());
                    inserted = true;
                }
                skipping = true;
                continue;
            }
            skipping = false;
        }
        if !skipping {
            out.push((*line).to_owned());
        }
    }

    if !inserted {
        while out.last().is_some_and(|l| l.is_empty()) {
            out.pop();
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        for blk_line in normalized_block.split(eol) {
            out.push(blk_line.to_owned());
        }
    }

    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let mut final_text = out.join(eol);
    final_text.push_str(eol);
    final_text
}

fn setup_cli_client(client: Client, cli_name: &str, opts: &SetupOptions) -> Result<ActionReport> {
    let bin_str = opts.binary.to_string_lossy().into_owned();
    let add_args: Vec<&str> = match client {
        Client::ClaudeCode => vec![
            "mcp",
            "add",
            "--scope",
            "user",
            "--transport",
            "stdio",
            SERVER_KEY,
            "--",
            &bin_str,
            "mcp",
        ],
        _ => unreachable!(),
    };

    let Some(cli_bin) = find_cli_on_path(cli_name) else {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "setup".into(),
            status: "not_found".into(),
            target: cli_name.into(),
            message: format!(
                "`{cli_name}` CLI not found on PATH. Run later: {cli_name} {}",
                add_args.join(" ")
            ),
        });
    };

    if opts.dry_run {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "setup".into(),
            status: "dry_run".into(),
            target: cli_bin.display().to_string(),
            message: format!("Would run: {cli_name} {}", add_args.join(" ")),
        });
    }

    let existing = Command::new(&cli_bin)
        .args(["mcp", "get", SERVER_KEY])
        .output();
    if let Ok(out) = existing
        && out.status.success()
    {
        if !opts.force {
            return Ok(ActionReport {
                client: client.id().into(),
                display_name: client.display_name().into(),
                action: "setup".into(),
                status: "preserved".into(),
                target: cli_bin.display().to_string(),
                message: format!(
                    "`{cli_name}` already has an `{SERVER_KEY}` MCP server (pass --force to replace)."
                ),
            });
        }
        let remove_args: &[&str] = match client {
            Client::ClaudeCode => &["mcp", "remove", "--scope", "user", SERVER_KEY],
            _ => unreachable!(),
        };
        let _ = Command::new(&cli_bin).args(remove_args).output();
    }

    let out = Command::new(&cli_bin)
        .args(&add_args)
        .output()
        .with_context(|| format!("execute `{cli_name}`"))?;
    ensure!(
        out.status.success(),
        "{cli_name} mcp add failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );

    Ok(ActionReport {
        client: client.id().into(),
        display_name: client.display_name().into(),
        action: "setup".into(),
        status: "configured".into(),
        target: cli_bin.display().to_string(),
        message: format!("Registered `{SERVER_KEY}` MCP server via `{cli_name}` CLI (global instruction files untouched)."),
    })
}

fn remove_cli_client(client: Client, cli_name: &str, opts: &SetupOptions) -> Result<ActionReport> {
    let Some(cli_bin) = find_cli_on_path(cli_name) else {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "not_found".into(),
            target: cli_name.into(),
            message: format!("`{cli_name}` CLI not found on PATH."),
        });
    };
    let remove_args: &[&str] = match client {
        Client::ClaudeCode => &["mcp", "remove", "--scope", "user", SERVER_KEY],
        _ => unreachable!(),
    };
    if opts.dry_run {
        return Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "dry_run".into(),
            target: cli_bin.display().to_string(),
            message: format!("Would run: {cli_name} {}", remove_args.join(" ")),
        });
    }
    let out = Command::new(&cli_bin).args(remove_args).output()?;
    if out.status.success() {
        Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "removed".into(),
            target: cli_bin.display().to_string(),
            message: format!("Removed `{SERVER_KEY}` from `{cli_name}`."),
        })
    } else {
        Ok(ActionReport {
            client: client.id().into(),
            display_name: client.display_name().into(),
            action: "remove".into(),
            status: "unchanged".into(),
            target: cli_bin.display().to_string(),
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

fn inspect_client(client: Client, home: &Path, check_system_apps: bool) -> (bool, String, String) {
    match client {
        Client::Antigravity => {
            let dir = antigravity_plugin_dir(home);
            let detected = home.join(".gemini").join("antigravity").is_dir()
                || home.join(".gemini").join("config").is_dir();
            (
                detected,
                dir.display().to_string(),
                "Native toggleable plugin bundle (plugin.json + mcp_config.json + progressive SKILL.md)".into(),
            )
        }
        Client::AntigravityMcp => {
            let file = antigravity_mcp_path(home);
            let detected = home.join(".gemini").join("config").is_dir();
            (
                detected,
                file.display().to_string(),
                "Global Antigravity MCP config (alternative to native plugin)".into(),
            )
        }
        Client::ClaudeCode => {
            let bin = find_cli_on_path("claude");
            (
                bin.is_some(),
                bin.map_or_else(|| "claude (not on PATH)".into(), |p| p.display().to_string()),
                "Registers via `claude mcp add --scope user`; leaves CLAUDE.md untouched".into(),
            )
        }
        Client::Codex => {
            let file = codex_config_path(home);
            let detected = file.is_file()
                || home.join(".codex").is_dir()
                || find_cli_on_path("codex").is_some()
                || (check_system_apps && macos_app_exists(&["ChatGPT.app", "Codex.app"]));
            (
                detected,
                file.display().to_string(),
                "ChatGPT Desktop & Codex CLI config (`[mcp_servers.omp]` in `~/.codex/config.toml`; leaves AGENTS.md untouched)".into(),
            )
        }
        Client::Cursor => {
            let dir = home.join(".cursor");
            let file = dir.join("mcp.json");
            (
                dir.is_dir(),
                file.display().to_string(),
                "User-level Cursor MCP config".into(),
            )
        }
        Client::VsCode => {
            let file = vscode_mcp_path(home, check_system_apps);
            let detected = file.parent().is_some_and(Path::is_dir);
            (
                detected,
                file.display().to_string(),
                "VS Code & GitHub Copilot user MCP config (`servers.omp`)".into(),
            )
        }
        Client::ClaudeDesktop => {
            let file = claude_desktop_config_path(home, check_system_apps);
            let detected = file.parent().is_some_and(Path::is_dir);
            (
                detected,
                file.display().to_string(),
                "Claude Desktop app config (restart app after setup)".into(),
            )
        }
        Client::Windsurf => {
            let file = home.join(".codeium").join("windsurf").join("mcp_config.json");
            let detected = file.parent().is_some_and(Path::is_dir);
            (
                detected,
                file.display().to_string(),
                "Windsurf user MCP config".into(),
            )
        }
        Client::Zed => {
            let file = zed_settings_path(home);
            let detected = file.parent().is_some_and(Path::is_dir);
            (
                detected,
                file.display().to_string(),
                "Use `--print zed` to copy `context_servers` snippet into JSONC settings".into(),
            )
        }
        Client::WorkBuddy => {
            let file = workbuddy_mcp_path(home);
            let detected = home.join(".workbuddy-ai").is_dir()
                || (check_system_apps && macos_app_exists(&["WorkBuddy.app"]));
            (
                detected,
                file.display().to_string(),
                "Tencent WorkBuddy MCP config (UI: Connectors -> Custom MCP; `--print workbuddy`)".into(),
            )
        }
        Client::CodeBuddy => {
            let file = codebuddy_mcp_path(home);
            let detected = home.join(".codebuddy").is_dir()
                || (check_system_apps
                    && (find_cli_on_path("codebuddy").is_some()
                        || macos_app_exists(&["CodeBuddy.app", "CodeBuddy IDE.app"])));
            (
                detected,
                file.display().to_string(),
                "Tencent CodeBuddy CLI / IDE user MCP config (`~/.codebuddy/.mcp.json`)".into(),
            )
        }
        Client::Trae => {
            let file = trae_mcp_path(home);
            let detected = home.join(".trae").is_dir()
                || home.join(".trae-cn").is_dir()
                || (check_system_apps && macos_app_exists(&["Trae.app", "Trae CN.app"]));
            (
                detected,
                file.display().to_string(),
                "ByteDance Trae / Trae CN global MCP config (`~/.trae/mcp.json`)".into(),
            )
        }
        Client::Qoder => {
            let file = qoder_settings_path(home);
            let detected = home.join(".qoder").is_dir()
                || (check_system_apps
                    && (find_cli_on_path("qoder").is_some()
                        || macos_app_exists(&["Qoder.app"])));
            (
                detected,
                file.display().to_string(),
                "Alibaba Cloud Qoder user config (merges `mcpServers.omp` into `~/.qoder/settings.json`)".into(),
            )
        }
        Client::Lingma => {
            let dir = home.join(".lingma");
            (
                dir.is_dir(),
                dir.display().to_string(),
                "Tongyi Lingma IDE plugin: run `--print lingma` and paste in Personal Settings -> MCP".into(),
            )
        }
        Client::Kiro => {
            let file = kiro_mcp_path(home);
            let detected = home.join(".kiro").is_dir()
                || (check_system_apps
                    && (find_cli_on_path("kiro-cli").is_some()
                        || find_cli_on_path("kiro").is_some()
                        || macos_app_exists(&["Kiro.app"])));
            (
                detected,
                file.display().to_string(),
                "AWS Kiro user MCP config (`~/.kiro/settings/mcp.json`)".into(),
            )
        }
        Client::Cline => {
            let file = cline_mcp_path(home, check_system_apps);
            let detected = file
                .parent()
                .and_then(Path::parent)
                .is_some_and(Path::is_dir);
            (
                detected,
                file.display().to_string(),
                "Cline VS Code extension MCP settings (`cline_mcp_settings.json`)".into(),
            )
        }
        Client::RooCode => {
            let file = roo_code_mcp_path(home, check_system_apps);
            let detected = file
                .parent()
                .and_then(Path::parent)
                .is_some_and(Path::is_dir);
            (
                detected,
                file.display().to_string(),
                "Roo Code VS Code extension MCP settings (`mcp_settings.json`)".into(),
            )
        }
    }
}

fn build_mcp_entry(binary: &Path, path_env: Option<String>) -> Value {
    let mut map = Map::new();
    map.insert("command".into(), json!(binary.to_string_lossy()));
    map.insert("args".into(), json!(["mcp"]));
    if let Some(p) = path_env {
        map.insert("env".into(), json!({ "PATH": p }));
    }
    Value::Object(map)
}

fn build_typed_stdio_mcp_entry(binary: &Path, path_env: Option<String>) -> Value {
    let mut entry = build_mcp_entry(binary, path_env);
    if let Value::Object(ref mut map) = entry {
        map.insert("type".into(), json!("stdio"));
    }
    entry
}

fn build_antigravity_plugin_manifest() -> Value {
    // Strictly stick to documented Antigravity top-level plugin.json fields.
    json!({
        "name": PLUGIN_DIR_NAME,
        "displayName": "OMP Code Intelligence",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Lean native Oh My Pi code intelligence: tree-sitter folded reading & block expansion, outlines, full-repo AST search/refactor, persistent LSP, persistent DAP debugging, Git conflict resolver, and CLI output minimization.",
        "repository": "https://github.com/y26s4824k264/omp-toolkit",
        "suggestedPrompts": [
            "Use code_outline and code_summary to inspect the architecture of this module without dumping full files",
            "Run ast_grep across the repository and preview an ast_edit refactor diff",
            "Check lsp diagnostics and references for the symbol I am changing",
            "Scan this repository with git_conflicts and preview structured resolutions"
        ]
    })
}

fn antigravity_skill_markdown() -> &'static str {
    r#"---
name: omp-code-intelligence
description: >-
  Use the local `omp` MCP server (extracted from Oh My Pi) for low-token tree-sitter code reading
  and syntactic block expansion (`code_summary`, `code_outline`), full-repository structural AST
  search and refactoring (`ast_grep`, `ast_edit`), persistent IDE-grade LSP diagnostics/navigation/rename
  (`lsp`), live DAP debugging (`debug`), noisy CLI log compression (`minimize_output`), and
  structured Git merge conflict resolution (`git_conflicts`).
---

# OMP Code Intelligence Workflow

Use the `omp` MCP server alongside native file/terminal tools — never fight native edits for simple small changes.

## When to reach for each tool

1. **Large files (>300 lines) — save context tokens first**
   - Call `code_outline` for instant tree-sitter symbol maps (functions, structs, classes, methods + exact line ranges).
   - Call `code_summary` (e.g. `unfold_until_lines: 120`) to see folded code structure with exact line numbers, or pass `block_line` to expand one full syntactic block (including Rust `#[...]` / `///` doc comments or Python/TS/Java decorators).
2. **Repo-wide structural search & refactors**
   - Call `ast_grep` with `root` + `patterns` (auto-detects languages and respects `.gitignore`; use `include`/`exclude` globs to narrow).
   - Call `ast_edit` with `apply: false` (default) first to inspect the unified diff and verify tree-sitter syntax checks; pass `apply: true` only after confirming the diff.
3. **IDE semantics & cross-file renames**
   - Call `lsp` (`status`, `diagnostics`, `definition`, `references`, `hover`, `type_definition`, `implementation`, `symbols`, `workspace_symbols`, `code_actions`).
   - Prefer passing `symbol` (or `symbol#occurrence` + `line`) instead of manually counting UTF-16 columns.
   - For `rename` and `rename_file`, preview the diff first (`apply: false`), then apply atomically with `apply: true`.
4. **Live debugging instead of print-statement guessing**
   - Call `debug` (`adapters` → `launch`/`attach` → `set_breakpoint` → `continue`/`step_*` → `stack_trace`/`scopes`/`variables`/`evaluate`/`output` → `terminate`).
5. **Noisy build/test logs**
   - Pass verbose `cargo`, `git`, `pytest`, `jest`, `tsc`, `go`, `docker`, or `gradle` outputs to `minimize_output` to surface failures cleanly.
6. **Git merge conflicts**
   - Call `git_conflicts` with `action: "scan"` to enumerate all 2-way and diff3 conflicts across the repo, then `action: "resolve"` (preview diff with `apply: false`, then write atomically with `apply: true`) using `ours`, `theirs`, `base`, `both`, `both_reverse`, `delete`, or `custom` (`@ours`/`@theirs`/`@base`/`@both`).
"#
}

fn resolved_path_env(opts: &SetupOptions) -> Option<String> {
    if !opts.include_path_env {
        return None;
    }
    if let Some(ref explicit) = opts.path_env {
        return (!explicit.trim().is_empty()).then(|| explicit.clone());
    }
    let current = env::var_os("PATH")?;
    let temp_root = env::temp_dir();
    let canonical_temp = temp_root.canonicalize().ok();
    let mut seen = BTreeSet::new();
    let mut kept = Vec::new();
    for dir in env::split_paths(&current) {
        if !dir.is_absolute() || !dir.is_dir() {
            continue;
        }
        if is_ephemeral_path(&dir, &temp_root, canonical_temp.as_deref()) {
            continue;
        }
        if seen.insert(dir.clone()) {
            kept.push(dir);
        }
    }
    env::join_paths(kept)
        .ok()
        .and_then(|s| s.into_string().ok())
        .filter(|s| !s.is_empty())
}

fn is_ephemeral_path(dir: &Path, temp_root: &Path, canonical_temp: Option<&Path>) -> bool {
    if dir.starts_with(temp_root) {
        return true;
    }
    if let Some(ct) = canonical_temp
        && dir.starts_with(ct)
    {
        return true;
    }
    let s = dir.to_string_lossy();
    s.starts_with("/tmp/")
        || s == "/tmp"
        || s.starts_with("/private/tmp/")
        || s == "/private/tmp"
        || s.contains("/T/agy-plugin-cli-")
        || s.contains("\\Temp\\")
}

fn resolve_home(override_home: Option<&Path>) -> Result<PathBuf> {
    if let Some(h) = override_home {
        return Ok(h.to_path_buf());
    }
    if let Some(h) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(h));
    }
    bail!("could not determine user home directory; pass --home explicitly")
}

fn antigravity_plugin_dir(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("config")
        .join("plugins")
        .join(PLUGIN_DIR_NAME)
}

fn antigravity_mcp_path(home: &Path) -> PathBuf {
    home.join(".gemini").join("config").join("mcp_config.json")
}

fn vscode_user_dir(home: &Path, use_env: bool) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = use_env;
        home.join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
    }
    #[cfg(target_os = "windows")]
    {
        let base = if use_env {
            env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData").join("Roaming"))
        } else {
            home.join("AppData").join("Roaming")
        };
        base.join("Code").join("User")
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let _ = use_env;
        home.join(".config").join("Code").join("User")
    }
}

fn vscode_mcp_path(home: &Path, use_env: bool) -> PathBuf {
    vscode_user_dir(home, use_env).join("mcp.json")
}

fn cline_mcp_path(home: &Path, use_env: bool) -> PathBuf {
    vscode_user_dir(home, use_env)
        .join("globalStorage")
        .join("saoudrizwan.claude-dev")
        .join("settings")
        .join("cline_mcp_settings.json")
}

fn roo_code_mcp_path(home: &Path, use_env: bool) -> PathBuf {
    vscode_user_dir(home, use_env)
        .join("globalStorage")
        .join("rooveterinaryinc.roo-cline")
        .join("settings")
        .join("mcp_settings.json")
}

fn claude_desktop_config_path(home: &Path, use_env: bool) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = use_env;
        home.join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json")
    }
    #[cfg(target_os = "windows")]
    {
        let base = if use_env {
            env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData").join("Roaming"))
        } else {
            home.join("AppData").join("Roaming")
        };
        base.join("Claude").join("claude_desktop_config.json")
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let _ = use_env;
        home.join(".config")
            .join("Claude")
            .join("claude_desktop_config.json")
    }
}

fn codex_config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

fn zed_settings_path(home: &Path) -> PathBuf {
    home.join(".config").join("zed").join("settings.json")
}

fn codebuddy_mcp_path(home: &Path) -> PathBuf {
    let dir = home.join(".codebuddy");
    let dot_mcp = dir.join(".mcp.json");
    let legacy = dir.join("mcp.json");
    if !dot_mcp.exists() && legacy.is_file() {
        legacy
    } else {
        dot_mcp
    }
}

fn workbuddy_mcp_path(home: &Path) -> PathBuf {
    let wb_dir = home.join(".workbuddy-ai");
    if wb_dir.is_dir() {
        wb_dir.join("mcp.json")
    } else {
        codebuddy_mcp_path(home)
    }
}

fn trae_mcp_path(home: &Path) -> PathBuf {
    let trae = home.join(".trae");
    let trae_cn = home.join(".trae-cn");
    if !trae.exists() && trae_cn.is_dir() {
        trae_cn.join("mcp.json")
    } else {
        trae.join("mcp.json")
    }
}

fn qoder_settings_path(home: &Path) -> PathBuf {
    home.join(".qoder").join("settings.json")
}

fn kiro_mcp_path(home: &Path) -> PathBuf {
    home.join(".kiro").join("settings").join("mcp.json")
}

fn macos_app_exists(names: &[&str]) -> bool {
    #[cfg(target_os = "macos")]
    {
        names.iter().any(|name| {
            Path::new("/Applications").join(name).is_dir()
                || env::var_os("HOME")
                    .map(PathBuf::from)
                    .is_some_and(|h| h.join("Applications").join(name).is_dir())
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = names;
        false
    }
}

fn find_cli_on_path(name: &str) -> Option<PathBuf> {
    let path_env = env::var_os("PATH")?;
    for dir in env::split_paths(&path_env) {
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{name}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
            let candidate = dir.join(name);
            if name.contains('.') && candidate.is_file() {
                return Some(candidate);
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_binary() -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\Tools\omp-toolkit.exe")
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/usr/local/bin/omp-toolkit")
        }
    }

    #[test]
    fn antigravity_plugin_setup_and_remove_preserves_other_configs() -> Result<()> {
        let fake_home = tempfile::tempdir()?;
        let home = fake_home.path();
        let bin = sample_binary();
        let bin_str = bin.to_string_lossy().into_owned();
        fs::create_dir_all(home.join(".gemini/config"))?;
        fs::create_dir_all(home.join(".cursor"))?;
        fs::write(
            home.join(".cursor/mcp.json"),
            r#"{"mcpServers":{"other":{"command":"foo","args":[]}}}"#,
        )?;

        let opts = SetupOptions {
            binary: bin,
            home: Some(home.to_path_buf()),
            path_env: Some("/usr/local/bin:/usr/bin".into()),
            dry_run: false,
            force: false,
            include_path_env: true,
        };

        let reports = apply_clients(&[Client::Antigravity, Client::Cursor, Client::VsCode], &opts)?;
        assert_eq!(reports[0].status, "configured");
        assert_eq!(reports[1].status, "configured");
        assert_eq!(reports[2].status, "configured");

        // Verify Antigravity plugin bundle files
        let plugin_dir = antigravity_plugin_dir(home);
        let manifest: Value = serde_json::from_str(&fs::read_to_string(plugin_dir.join("plugin.json"))?)?;
        assert_eq!(manifest["name"], "omp-toolkit");
        assert_eq!(manifest["suggestedPrompts"].as_array().unwrap().len(), 4);
        let agy_mcp: Value = serde_json::from_str(&fs::read_to_string(plugin_dir.join("mcp_config.json"))?)?;
        assert_eq!(agy_mcp["mcpServers"]["omp"]["command"], bin_str);
        assert_eq!(agy_mcp["mcpServers"]["omp"]["env"]["PATH"], "/usr/local/bin:/usr/bin");
        assert!(plugin_dir.join("skills/omp-code-intelligence/SKILL.md").is_file());

        // Verify Cursor preserved sibling server
        let cursor_mcp: Value = serde_json::from_str(&fs::read_to_string(home.join(".cursor/mcp.json"))?)?;
        assert_eq!(cursor_mcp["mcpServers"]["other"]["command"], "foo");
        assert_eq!(cursor_mcp["mcpServers"]["omp"]["command"], bin_str);

        // Verify VS Code used `servers` + `type: stdio`
        let vscode_mcp: Value = serde_json::from_str(&fs::read_to_string(vscode_mcp_path(home, false))?)?;
        assert_eq!(vscode_mcp["servers"]["omp"]["type"], "stdio");

        // Verify clean removal leaves sibling server intact
        let removed = remove_clients(&[Client::Antigravity, Client::Cursor], &opts)?;
        assert_eq!(removed[0].status, "removed");
        assert!(!plugin_dir.exists());
        assert_eq!(removed[1].status, "removed");
        let cursor_after: Value = serde_json::from_str(&fs::read_to_string(home.join(".cursor/mcp.json"))?)?;
        assert!(cursor_after["mcpServers"].get("omp").is_none());
        assert_eq!(cursor_after["mcpServers"]["other"]["command"], "foo");

        // Verify zero global instruction files created
        assert!(!home.join(".claude/CLAUDE.md").exists());
        assert!(!home.join(".codex/AGENTS.md").exists());
        assert!(!home.join(".gemini/config/config.json").exists());
        Ok(())
    }

    #[test]
    fn preserves_unrelated_existing_omp_entry_without_force() -> Result<()> {
        let fake_home = tempfile::tempdir()?;
        let home = fake_home.path();
        fs::create_dir_all(home.join(".cursor"))?;
        fs::write(
            home.join(".cursor/mcp.json"),
            r#"{"mcpServers":{"omp":{"command":"/custom/other-omp","args":[]}}}"#,
        )?;
        let opts = SetupOptions {
            binary: sample_binary(),
            home: Some(home.to_path_buf()),
            path_env: None,
            dry_run: false,
            force: false,
            include_path_env: false,
        };
        let rep = apply_clients(&[Client::Cursor], &opts)?;
        assert_eq!(rep[0].status, "preserved");

        let forced = SetupOptions { force: true, ..opts };
        let rep2 = apply_clients(&[Client::Cursor], &forced)?;
        assert_eq!(rep2[0].status, "configured");
        Ok(())
    }

    #[test]
    fn codex_chatgpt_toml_upgrades_legacy_omp_and_preserves_comments() -> Result<()> {
        let fake_home = tempfile::tempdir()?;
        let home = fake_home.path();
        let bin = sample_binary();
        let bin_str = bin.to_string_lossy().into_owned();
        fs::create_dir_all(home.join(".codex"))?;
        let original = "\
# User top comment\n\
model = \"gpt-6.2-sol\"\n\
\n\
[mcp_servers.other]\n\
command = \"demo\"\n\
args = []\n\
\n\
[mcp_servers.omp]\n\
command = \"/old/.local/omp-toolkit/bin/omp-toolkit-mcp\"\n\
args = [\"--bin-dir\", \"/old\"]\n\
omit_tools_from = [\"direct\"]\n\
\n\
[desktop]\n\
# Keep desktop comment\n\
sansFontSize = 14\n";
        fs::write(home.join(".codex/config.toml"), original)?;

        let opts = SetupOptions {
            binary: bin,
            home: Some(home.to_path_buf()),
            path_env: Some("/usr/local/bin:/usr/bin".into()),
            dry_run: false,
            force: false,
            include_path_env: true,
        };

        let rep = apply_clients(&[Client::Codex], &opts)?;
        assert_eq!(rep[0].status, "configured");
        let after = fs::read_to_string(home.join(".codex/config.toml"))?;
        assert!(after.contains("# User top comment"), "{after}");
        assert!(after.contains("# Keep desktop comment"), "{after}");
        assert!(!after.contains("omit_tools_from"), "{after}");

        let parsed: toml::Value = toml::from_str(&after)?;
        assert_eq!(parsed["mcp_servers"]["omp"]["command"].as_str(), Some(bin_str.as_str()));
        assert_eq!(parsed["mcp_servers"]["omp"]["args"][0].as_str(), Some("mcp"));
        assert_eq!(
            parsed["mcp_servers"]["omp"]["env"]["PATH"].as_str(),
            Some("/usr/local/bin:/usr/bin")
        );
        assert_eq!(parsed["mcp_servers"]["other"]["command"].as_str(), Some("demo"));
        assert_eq!(parsed["desktop"]["sansFontSize"].as_integer(), Some(14));

        // Clean removal preserves comments and sibling servers
        let rem = remove_clients(&[Client::Codex], &opts)?;
        assert_eq!(rem[0].status, "removed");
        let cleaned = fs::read_to_string(home.join(".codex/config.toml"))?;
        assert!(cleaned.contains("# User top comment"));
        assert!(cleaned.contains("# Keep desktop comment"));
        let parsed_clean: toml::Value = toml::from_str(&cleaned)?;
        assert!(parsed_clean["mcp_servers"].get("omp").is_none());
        assert_eq!(parsed_clean["mcp_servers"]["other"]["command"].as_str(), Some("demo"));
        Ok(())
    }

    #[test]
    fn china_and_extension_clients_setup_and_remove_preserve_settings() -> Result<()> {
        let fake_home = tempfile::tempdir()?;
        let home = fake_home.path();
        let bin = sample_binary();
        let bin_str = bin.to_string_lossy().into_owned();

        // Prepare realistic existing directories & settings for Chinese + extension clients
        fs::create_dir_all(home.join(".workbuddy-ai"))?;
        fs::create_dir_all(home.join(".codebuddy"))?;
        fs::create_dir_all(home.join(".trae"))?;
        fs::create_dir_all(home.join(".qoder"))?;
        fs::write(
            home.join(".qoder/settings.json"),
            r#"{"theme":"dark","mcpServers":{"keep":{"command":"demo","args":[]}}}"#,
        )?;
        fs::create_dir_all(home.join(".lingma"))?;
        fs::create_dir_all(home.join(".kiro/settings"))?;
        fs::create_dir_all(cline_mcp_path(home, false).parent().unwrap())?;
        fs::create_dir_all(roo_code_mcp_path(home, false).parent().unwrap())?;

        let opts = SetupOptions {
            binary: bin,
            home: Some(home.to_path_buf()),
            path_env: Some("/usr/local/bin:/usr/bin".into()),
            dry_run: false,
            force: false,
            include_path_env: true,
        };

        let targets = [
            Client::WorkBuddy,
            Client::CodeBuddy,
            Client::Trae,
            Client::Qoder,
            Client::Lingma,
            Client::Kiro,
            Client::Cline,
            Client::RooCode,
        ];
        let reports = apply_clients(&targets, &opts)?;
        assert_eq!(reports[0].status, "configured"); // WorkBuddy -> ~/.workbuddy-ai/mcp.json
        assert_eq!(reports[1].status, "configured"); // CodeBuddy -> ~/.codebuddy/.mcp.json
        assert_eq!(reports[2].status, "configured"); // Trae -> ~/.trae/mcp.json
        assert_eq!(reports[3].status, "configured"); // Qoder -> ~/.qoder/settings.json
        assert_eq!(reports[4].status, "skipped"); // Lingma -> manual UI paste
        assert_eq!(reports[5].status, "configured"); // Kiro -> ~/.kiro/settings/mcp.json
        assert_eq!(reports[6].status, "configured"); // Cline
        assert_eq!(reports[7].status, "configured"); // Roo Code

        // WorkBuddy & CodeBuddy both write `type: stdio`
        let wb: Value = serde_json::from_str(&fs::read_to_string(home.join(".workbuddy-ai/mcp.json"))?)?;
        assert_eq!(wb["mcpServers"]["omp"]["type"], "stdio");
        let cb: Value = serde_json::from_str(&fs::read_to_string(home.join(".codebuddy/.mcp.json"))?)?;
        assert_eq!(cb["mcpServers"]["omp"]["type"], "stdio");

        // Qoder preserves non-MCP keys (`theme`) and sibling servers (`keep`)
        let qoder: Value = serde_json::from_str(&fs::read_to_string(home.join(".qoder/settings.json"))?)?;
        assert_eq!(qoder["theme"], "dark");
        assert_eq!(qoder["mcpServers"]["keep"]["command"], "demo");
        assert_eq!(qoder["mcpServers"]["omp"]["type"], "stdio");

        // Trae, Kiro, Cline, Roo Code
        let trae: Value = serde_json::from_str(&fs::read_to_string(home.join(".trae/mcp.json"))?)?;
        assert_eq!(trae["mcpServers"]["omp"]["command"], bin_str);
        let kiro: Value = serde_json::from_str(&fs::read_to_string(home.join(".kiro/settings/mcp.json"))?)?;
        assert_eq!(kiro["mcpServers"]["omp"]["command"], bin_str);
        let cline: Value = serde_json::from_str(&fs::read_to_string(cline_mcp_path(home, false))?)?;
        assert_eq!(cline["mcpServers"]["omp"]["command"], bin_str);
        let roo: Value = serde_json::from_str(&fs::read_to_string(roo_code_mcp_path(home, false))?)?;
        assert_eq!(roo["mcpServers"]["omp"]["command"], bin_str);

        // Clean removal from Qoder leaves `theme` and `keep` untouched
        let rem = remove_clients(&[Client::Qoder, Client::WorkBuddy, Client::Trae], &opts)?;
        assert_eq!(rem[0].status, "removed");
        assert_eq!(rem[1].status, "removed");
        assert_eq!(rem[2].status, "removed");
        let qoder_after: Value = serde_json::from_str(&fs::read_to_string(home.join(".qoder/settings.json"))?)?;
        assert_eq!(qoder_after["theme"], "dark");
        assert_eq!(qoder_after["mcpServers"]["keep"]["command"], "demo");
        assert!(qoder_after["mcpServers"].get("omp").is_none());

        // Snippet printing works for Lingma & WorkBuddy
        let lingma_snippet = render_snippet(Client::Lingma, &opts)?;
        assert!(lingma_snippet.contains("\"mcpServers\""));
        Ok(())
    }
}
