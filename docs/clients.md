# Supported AI Coding Clients & Setup Guide

`omp-toolkit` includes a built-in, zero-pollution adapter (`omp-toolkit setup`) that connects the single binary to mainstream AI coding software without modifying global system prompt files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`) or overwriting unrelated MCP servers.

## Supported Software Matrix (17 Targets)

| Client | `--client` ID | Integration Type | Target Path / Command | Auto-detected by `auto` |
| --- | --- | --- | --- | --- |
| **Google Antigravity (AGY)** | `antigravity` *(recommended)* | Native toggleable plugin (`plugin.json` + `mcp_config.json` + progressive `SKILL.md`) | `~/.gemini/config/plugins/omp-toolkit/` | Yes |
| **Google Antigravity (MCP only)** | `antigravity-mcp` | Global Stdio MCP | `~/.gemini/config/mcp_config.json` | No *(explicit)* |
| **WorkBuddy (Tencent)** | `workbuddy` | User Stdio MCP (`type: stdio`) | `~/.workbuddy-ai/mcp.json` (if `~/.workbuddy-ai` exists) else shared `~/.codebuddy/.mcp.json` | Yes |
| **CodeBuddy (Tencent)** | `codebuddy` | User Stdio MCP (`type: stdio`) | `~/.codebuddy/.mcp.json` (or existing `~/.codebuddy/mcp.json`) | Yes |
| **Trae / Trae CN (ByteDance)** | `trae` | User Stdio MCP | `~/.trae/mcp.json` (or `~/.trae-cn/mcp.json`) | Yes |
| **Qoder (Alibaba Cloud)** | `qoder` | Settings JSON merge (`type: stdio`) | `~/.qoder/settings.json` (`mcpServers.omp`, preserves all other settings) | Yes |
| **Tongyi Lingma (Alibaba)** | `lingma` | JSON snippet (`--print lingma`) | IDE Sidebar → **Personal Settings → MCP → Manual JSON** | Manual paste |
| **Claude Code** | `claude-code` | Native CLI registration | `claude mcp add --scope user --transport stdio omp -- <bin> mcp` | Yes |
| **ChatGPT Desktop / Codex** | `codex` *(alias: `chatgpt`)* | Comment-preserving TOML update | `~/.codex/config.toml` (`[mcp_servers.omp]` + `[mcp_servers.omp.env]`) | Yes |
| **Cursor** | `cursor` | User Stdio MCP | `~/.cursor/mcp.json` (`mcpServers.omp`) | Yes |
| **VS Code / GitHub Copilot** | `vscode` | User Stdio MCP | macOS: `~/Library/Application Support/Code/User/mcp.json`<br>Linux: `~/.config/Code/User/mcp.json`<br>Windows: `%APPDATA%\Code\User\mcp.json` (`servers.omp`) | Yes |
| **Claude Desktop** | `claude-desktop` | App Stdio MCP | macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`<br>Linux: `~/.config/Claude/claude_desktop_config.json`<br>Windows: `%APPDATA%\Claude\claude_desktop_config.json` | Yes |
| **Windsurf** | `windsurf` | User Stdio MCP | `~/.codeium/windsurf/mcp_config.json` (`mcpServers.omp`) | Yes |
| **Kiro (AWS)** | `kiro` | User Stdio MCP | `~/.kiro/settings/mcp.json` (`mcpServers.omp`) | Yes |
| **Cline (VS Code)** | `cline` | Extension Stdio MCP | `Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` | Yes |
| **Roo Code (VS Code)** | `roo-code` | Extension Stdio MCP | `Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json` | Yes |
| **Zed** | `zed` | JSONC snippet (`--print zed`) | `~/.config/zed/settings.json` (`context_servers.omp`) | Manual paste |
| **Any other stdio MCP client** | — | Standard Stdio MCP | `command: "<bin>"`, `args: ["mcp"]` | Manual |

---

## One-Click Setup Commands

### 1. Inspect detected clients (read-only)

```sh
omp-toolkit setup --client list
```

### 2. Preview changes before touching disk

```sh
omp-toolkit setup --client auto --dry-run
```

### 3. Build, install binary to `~/.local/bin`, and configure in one command

```sh
# Antigravity (installs toggleable native plugin)
./install.sh --client antigravity

# Multiple clients at once
./install.sh --client antigravity --client claude-code --client cursor

# Auto-configure every detected client
./install.sh --client auto
```

On Windows PowerShell:

```powershell
.\install.ps1 -Client antigravity
.\install.ps1 -Client auto -DryRun
```

### 4. Print a ready-to-paste snippet without writing anything

```sh
omp-toolkit setup --print antigravity
omp-toolkit setup --print workbuddy
omp-toolkit setup --print trae
omp-toolkit setup --print qoder
omp-toolkit setup --print lingma
omp-toolkit setup --print cursor
omp-toolkit setup --print vscode
omp-toolkit setup --print zed
omp-toolkit setup --print claude-code
```

### 5. Clean uninstall / rollback

```sh
# Remove from specific client(s), keeping all sibling MCP servers untouched
omp-toolkit setup --client antigravity --remove
omp-toolkit setup --client cursor --client vscode --remove

# Or via install script without rebuilding
./install.sh --skip-build --client auto --remove
```

---

## Client-Specific Notes

### Google Antigravity (AGY)

Two modes are supported:

1. **Native Plugin (`--client antigravity`, recommended)**:
   Creates `~/.gemini/config/plugins/omp-toolkit/` with:
   - `plugin.json`: standard Antigravity plugin manifest with starter prompts.
   - `mcp_config.json`: connects `omp-toolkit mcp` over stdio with `env.PATH` preserved for GUI launches.
   - `skills/omp-code-intelligence/SKILL.md`: progressive-disclosure skill (only loaded when relevant, near-zero idle context overhead).
   - **Live toggle**: Enable, disable, or inspect the bundle from Antigravity's **Customizations** page without restarting. `omp-toolkit` never hand-edits `~/.gemini/config/config.json`.
2. **Global MCP (`--client antigravity-mcp`)**:
   Merges only `mcpServers.omp` into `~/.gemini/config/mcp_config.json` if you prefer managing MCP outside plugins.

### ChatGPT Desktop App & OpenAI Codex

Both the ChatGPT Desktop App (macOS / Windows) and Codex CLI read user-level local MCP servers from `~/.codex/config.toml`. Running `--client codex` (or `--client chatgpt`) updates `[mcp_servers.omp]` and `[mcp_servers.omp.env]` atomically **while preserving all comments, formatting, and unrelated sections**, validates the resulting TOML before writing, and leaves `~/.codex/AGENTS.md` untouched.

### Chinese AI Coding Clients (WorkBuddy, CodeBuddy, Trae, Qoder, Tongyi Lingma)

- **WorkBuddy & CodeBuddy (Tencent)**:
  - WorkBuddy shares CodeBuddy's core engine. When `~/.workbuddy-ai` exists, `--client workbuddy` updates `~/.workbuddy-ai/mcp.json`; otherwise it writes to the shared `~/.codebuddy/.mcp.json` (or existing `~/.codebuddy/mcp.json`) with `"type": "stdio"`.
  - If you prefer configuring inside WorkBuddy's desktop UI (**Connectors / 连接器 → Custom Connector / 自定义连接器 → Add MCP**), run `omp-toolkit setup --print workbuddy` and paste the JSON directly.
- **Trae / Trae CN (ByteDance)**:
  - Writes `~/.trae/mcp.json` (or `~/.trae-cn/mcp.json` if only `.trae-cn` exists). You can also copy the output of `omp-toolkit setup --print trae` into Trae's **Settings → MCP → Manual / Raw Config** or a project-level `.trae/mcp.json`.
- **Qoder (Alibaba Cloud)**:
  - Atomically merges `mcpServers.omp` into `~/.qoder/settings.json` without disturbing any existing IDE preferences, themes, or sibling MCP servers.
- **Tongyi Lingma (Alibaba)**:
  - Because the IDE plugin manages internal encrypted/synced state under `~/.lingma`, `omp-toolkit` avoids blind file writes. Run `omp-toolkit setup --print lingma` and paste into **Personal Settings (个人设置) → MCP → Manual JSON**.

### GUI App `PATH` Inheritance (macOS / Linux)

Desktop apps (Antigravity App, WorkBuddy, Trae, Qoder, Cursor, VS Code, Claude Desktop, Windsurf, Kiro) launched from Finder/Dock often do **not** inherit shell PATH additions from `~/.zshrc` or `~/.bashrc` (such as `~/.cargo/bin`, `/opt/homebrew/bin`, `go/bin`, or `nvm`/`pyenv` shims).

By default, `omp-toolkit setup` snapshots the sanitized, deduplicated directories from your current terminal `PATH` into `"env": { "PATH": "..." }` inside GUI MCP configs so warm LSP servers (`rust-analyzer`, `gopls`, `pyright`, `typescript-language-server`) and DAP adapters (`lldb-dap`, `dlv`, `debugpy`) are always discoverable. Pass `--no-path-env` if you want to omit `env.PATH`.

### Conflict Safety

- If a target config already contains an `omp` entry pointing to a different command, `omp-toolkit setup` **preserves it** and reports `[preserved]`. Pass `--force` to replace it explicitly.
- If a target JSON file contains syntax errors or comments (e.g. Zed's JSONC `settings.json`), `omp-toolkit setup` refuses to overwrite it to prevent data loss.
