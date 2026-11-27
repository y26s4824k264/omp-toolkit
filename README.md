# OMP Toolkit

English · [简体中文](README.zh-CN.md)

A lean, single-binary Rust extraction of [Oh My Pi](https://github.com/can1357/oh-my-pi)'s highest-value code intelligence engines — natively supporting **Windows**, **macOS**, and **Linux** (`x64` & `arm64`), and plugging cleanly into **Antigravity (AGY)**, **ChatGPT Desktop / Codex**, **WorkBuddy**, **CodeBuddy**, **Trae**, **Qoder**, **Tongyi Lingma**, **Claude Code**, **Cursor**, **VS Code / GitHub Copilot**, **Claude Desktop**, **Windsurf**, **Kiro**, **Cline**, **Roo Code**, **Zed**, and any stdio MCP client.

- **Cross-platform single ~20 MB binary**: macOS (`arm64`/`x64`), Linux (`x64`/`arm64`), and Windows (`x64`/`arm64`) — no Node.js runtime, no 94 MB embedded bash, no forced npm language-server installs.
- **In-process MCP & CLI**: zero subprocess forking inside MCP, zero `/tmp` patch leaks.
- **Non-intrusive by design**: never edits your global `CLAUDE.md`, `AGENTS.md`, or `GEMINI.md`; never competes with your agent's native `Edit` or `Bash`; structural rewrites and conflict resolutions default to dry-run unified diff preview (`apply: false`).
- **Built-in one-click setup & rollback**: `omp-toolkit setup` configures or cleanly removes integrations across 17 supported international & Chinese AI clients.

---

## Core Capabilities (8 Orthogonal MCP Tools)

| Tool | Read / Write | What it extracts from Oh My Pi (`v19.1.0`) |
| --- | --- | --- |
| `code_summary` | Read-only | Tree-sitter AST-folded reading with progressive BFS unfolding (`unfold_until_lines`) **and** syntactic block expansion (`block_line`, including Rust `#[...]`/`///` doc comments & Python/TS/Java decorators) — cuts large-file read tokens by 60–80% while keeping exact line anchors. |
| `code_outline` | Read-only | Instant tree-sitter symbol map (functions, structs, classes, interfaces, methods & exact line ranges) with zero LSP startup cost. |
| `ast_grep` | Read-only | Full-repository parallel AST search (`omp-ast` + `omp-xfs`) with `.gitignore`/worktree awareness, globs, automatic per-file language detection, multi-pattern captures, and pagination. |
| `ast_edit` | Dry-run default | Full-repository parallel AST structural rewrites with mandatory post-rewrite tree-sitter syntax checks. Previews unified diff (`apply: false`) or writes atomically (`apply: true`). |
| `lsp` | Persistent pool | Cross-platform warm LSP pool (auto-syncs external edits made by your host agent). Bundles OMP's 55-server `defaults.json` (+ `.omp/lsp.json` overrides). Supports `status`, `diagnostics`, `definition`, `references`, `hover`, `type_definition`, `implementation`, `symbols`, `workspace_symbols`, `code_actions`, `rename`, and `rename_file` (`workspace/willRenameFiles`). |
| `debug` | Persistent sessions | Cross-platform multi-session DAP debugger using OMP's 14-adapter `defaults.json` (both stdio & TCP socket adapters like `dlv`/`codelldb`). Supports `launch`, `attach`, conditional breakpoints, stepping, `threads`, `stack_trace`, `scopes`, `variables`, `evaluate`, `output`, and `terminate`. |
| `minimize_output` | Read-only | Standalone extraction of OMP's command-aware output minimizer (20+ Rust filters + 67 TOML pipelines for `cargo`, `git`, `pytest`, `jest`, `tsc`, `go`, `docker`, `gradle`, etc.) — works on non-zero exit codes without bundling bash. |
| `git_conflicts` | Dry-run default | Inspired by OMP's `conflict://` workflow: parallel full-repo scan for 2-way & diff3 3-way Git conflict markers, plus structured batch resolution (`ours`, `theirs`, `base`, `both`, `both_reverse`, `delete`, or `custom` with `@ours`/`@theirs`/`@base`/`@both` tokens), CRLF-safe, diff preview by default. |

---

## Supported AI Coding Software (17 Targets)

| Software | One-Click Command | Integration Method |
| --- | --- | --- |
| **Google Antigravity (AGY)** | `./install.sh --client antigravity` | Native toggleable plugin at `~/.gemini/config/plugins/omp-toolkit/` (`plugin.json` + `mcp_config.json` + progressive `SKILL.md`) |
| **Antigravity (MCP only)** | `./install.sh --client antigravity-mcp` | Global `~/.gemini/config/mcp_config.json` |
| **WorkBuddy (Tencent)** | `./install.sh --client workbuddy` | `~/.workbuddy-ai/mcp.json` (or shared `~/.codebuddy/.mcp.json`) with `type: stdio`; also `--print workbuddy` for UI Connectors |
| **CodeBuddy (Tencent)** | `./install.sh --client codebuddy` | `~/.codebuddy/.mcp.json` (`mcpServers.omp`, `type: stdio`) |
| **Trae / Trae CN (ByteDance)** | `./install.sh --client trae` | `~/.trae/mcp.json` (or `~/.trae-cn/mcp.json`) |
| **Qoder (Alibaba Cloud)** | `./install.sh --client qoder` | Merges `mcpServers.omp` into `~/.qoder/settings.json` (preserves all other settings) |
| **Tongyi Lingma (Alibaba)** | `omp-toolkit setup --print lingma` | Prints `mcpServers` JSON for IDE sidebar **Personal Settings → MCP → Manual** |
| **Claude Code** | `./install.sh --client claude-code` | `claude mcp add --scope user --transport stdio omp -- <bin> mcp` |
| **ChatGPT Desktop / Codex** | `./install.sh --client codex` *(or `chatgpt`)* | Comment-preserving atomic TOML update of `[mcp_servers.omp]` in `~/.codex/config.toml` (works even without `codex` on PATH) |
| **Cursor** | `./install.sh --client cursor` | `~/.cursor/mcp.json` (`mcpServers.omp`) |
| **VS Code / GitHub Copilot** | `./install.sh --client vscode` | User `Code/User/mcp.json` (`servers.omp`) |
| **Claude Desktop** | `./install.sh --client claude-desktop` | `claude_desktop_config.json` (`mcpServers.omp`) |
| **Windsurf** | `./install.sh --client windsurf` | `~/.codeium/windsurf/mcp_config.json` (`mcpServers.omp`) |
| **Kiro (AWS)** | `./install.sh --client kiro` | `~/.kiro/settings/mcp.json` (`mcpServers.omp`) |
| **Cline (VS Code)** | `./install.sh --client cline` | `globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` |
| **Roo Code (VS Code)** | `./install.sh --client roo-code` | `globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json` |
| **Zed** | `omp-toolkit setup --print zed` | Prints `context_servers.omp` JSONC snippet |

> Full client paths, GUI `PATH` handling, and rollback instructions: [docs/clients.md](docs/clients.md)

---

## Quick Start

Requires Rust (the pinned nightly in [rust-toolchain.toml](rust-toolchain.toml) is needed by upstream `xutf`'s `portable_simd`).

### Build & install binary only

```sh
./install.sh
# Binary installed to ~/.local/bin/omp-toolkit
```

### Inspect detected AI clients & preview changes (no disk writes)

```sh
omp-toolkit setup --client list
omp-toolkit setup --client auto --dry-run
```

### One-click build, install & connect

```sh
# macOS / Linux
./install.sh --client antigravity
./install.sh --client antigravity --client claude-code --client cursor
./install.sh --client auto
```

```powershell
# Windows PowerShell
.\install.ps1 -Client antigravity
.\install.ps1 -Client auto -DryRun
```

### Clean uninstall from any client

```sh
omp-toolkit setup --client antigravity --remove
omp-toolkit setup --client cursor --remove
```

Install only the language servers or debug adapters you actually use on your `PATH` (e.g. `rust-analyzer`, `typescript-language-server`, `pyright`, `gopls`, `lldb-dap`, `dlv`, `debugpy`).

---

## CLI Examples

```sh
# Fold a large source file to ~120 visible lines, or expand one full syntactic block
omp-toolkit summary src/code_ast.rs --unfold-until 120
omp-toolkit summary src/code_ast.rs --block-line 135

# Instant tree-sitter symbol outline
omp-toolkit outline src/mcp.rs --type function

# Repo-wide AST search (auto-detects languages, respects .gitignore)
omp-toolkit ast-grep --pattern 'oldCall($$$ARGS)' --include 'src/**/*.ts'

# Repo-wide AST rewrite preview (add --apply to write atomically)
omp-toolkit ast-edit --pattern 'oldCall($$$ARGS)' --rewrite 'newCall($$$ARGS)'

# Scan & resolve Git merge conflicts (preview diff by default; add --apply to write)
omp-toolkit conflicts scan
omp-toolkit conflicts resolve src/app.rs --index 1 --choice ours

# Check project LSP readiness & available DAP adapters
omp-toolkit lsp status
omp-toolkit debug-adapters

# Condense noisy build or failing test output
cargo test 2>&1 | omp-toolkit minimize --cmd "cargo test" --exit-code 101
```

---

## Documentation

- [docs/clients.md](docs/clients.md) — Supported AI software, one-click setup, GUI `PATH` tips, and clean uninstall
- [docs/tools.md](docs/tools.md) — Complete MCP tool schemas and CLI reference
- [docs/architecture.md](docs/architecture.md) — Extraction philosophy, crate layout, and engineering decisions
- [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) — Upstream Oh My Pi (`v19.1.0`) and RTK MIT license attributions

## License

MIT — see [LICENSE](LICENSE).
