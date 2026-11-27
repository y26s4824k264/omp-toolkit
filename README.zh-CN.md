# OMP Toolkit

[English](README.md) · 简体中文

从 [Oh My Pi (OMP)](https://github.com/can1357/oh-my-pi) 重新精简抽取的**纯 Rust 单二进制**代码智能工具箱，原生支持 **Windows / macOS / Linux**（`x64` & `arm64`），一键适配 **Antigravity（反重力）**、**ChatGPT 桌面端 / Codex**、**WorkBuddy**、**CodeBuddy**、**Trae / Trae CN**、**Qoder**、**通义灵码**、**Claude Code**、**Cursor**、**VS Code / GitHub Copilot**、**Claude Desktop**、**Windsurf**、**Kiro**、**Cline**、**Roo Code**、**Zed** 及任意 stdio MCP 客户端。

- **三端单二进制约 20 MB**：完整支持 macOS（`arm64`/`x64`）、Linux（`x64`/`arm64`）、Windows（`x64`/`arm64`），无需 Node.js 运行时，不打包 94 MB 嵌入式 bash，不强制预装 npm 语言服务器。
- **MCP 与 CLI 进程内直调**：零子进程 fork 套娃，零 `/tmp` 临时 patch 泄漏。
- **对宿主 AI 零干扰、零污染**：绝不修改全局 `CLAUDE.md`、`AGENTS.md` 或 `GEMINI.md`；不抢占宿主原生 `Edit`/`Bash`；涉及代码改写与冲突解决的操作默认只读预览 unified diff（`apply: false`）。
- **内置跨平台一键适配与卸载**：`omp-toolkit setup` 支持 17 种国内外主流客户端的自动检测、演练预览（`--dry-run`）、一键接入和无痕卸载（`--remove`）。

---

## 核心能力（8 个正交 MCP 工具）

| MCP 工具 | 读写模式 | 抽取自 OMP (`v19.1.0`) 的精华能力 |
| --- | --- | --- |
| `code_summary` | 只读 | Tree-sitter AST 智能折叠阅读（按 `unfold_until_lines` 预算渐进 BFS 展开）+ **精准语法块展开（`block_line`）**（自动包含 Rust `#[...]`/`///` 注释、Python/TS/Java 装饰器），读大文件省 60%–80% token 且保留精确行号。 |
| `code_outline` | 只读 | Tree-sitter 毫秒级符号大纲（函数、结构体、类、接口、方法及精确起止行号），无需等待 LSP 启动。 |
| `ast_grep` | 只读 | 全仓并行 AST 结构搜索（`omp-ast` + `omp-xfs`），自动遵守 `.gitignore`/worktree exclude、自动推断语言、支持 glob、多 pattern、元变量捕获与分页。 |
| `ast_edit` | 默认预览 | 全仓并行 AST 结构化改写，改写后强制执行 tree-sitter 语法校验；默认 `apply: false` 返回 unified diff，`apply: true` 原子写盘。 |
| `lsp` | 常驻连接池 | 跨平台常驻 LSP 连接池（复用预热语言服务器，每次查询前自动同步宿主 Agent 的外部磁盘改写）。内置 OMP 55 种语言服务器配置（支持 `.omp/lsp.json` 覆盖），支持 `status`、`diagnostics`、`definition`、`references`、`hover`、`type_definition`、`implementation`、`symbols`、`workspace_symbols`、`code_actions`、`rename`、`rename_file`（走 `workspace/willRenameFiles` 自动更新 import）。 |
| `debug` | 常驻多会话 | 跨平台常驻多会话 DAP 调试器，内置 OMP 14 种适配器配置（同时支持 stdio 与 TCP socket 如 `dlv`/`codelldb`），完整支持 `launch`、`attach`、条件断点、单步、`threads`、`stack_trace`、`scopes`、`variables`、`evaluate`、`output`、`terminate`。 |
| `minimize_output` | 只读 | 从 OMP `crates/shell` 独立剥离的命令输出压缩器（20+ Rust 过滤器 + 67 个内置 TOML 管道，覆盖 `cargo`、`git`、`pytest`、`jest`、`tsc`、`go`、`docker`、`gradle` 等），不含 bash 引擎，且支持非 0 退出码失败日志摘要。 |
| `git_conflicts` | 默认预览 | 复刻 OMP `conflict://` 理念：基于 `omp-xfs` 并行扫描全仓 2-way 与 diff3 3-way Git 冲突标记，支持按编号/批量结构化解决（`ours`/`theirs`/`base`/`both`/`both_reverse`/`delete`/`custom` 并支持 `@ours`/`@theirs`/`@base`/`@both` 占位行展开），完美保留 CRLF，默认 diff 预览、`apply: true` 原子写盘。 |

---

## 支持的 AI 编程软件一览（17 种目标）

| 软件 | 一键适配命令 | 适配方式与写入位置 |
| --- | --- | --- |
| **Google Antigravity（反重力）** | `./install.sh --client antigravity` | 生成原生可一键开关的插件包 `~/.gemini/config/plugins/omp-toolkit/`（`plugin.json` + `mcp_config.json` + 按需加载 `SKILL.md`） |
| **Antigravity（仅全局 MCP）** | `./install.sh --client antigravity-mcp` | 原子合并到 `~/.gemini/config/mcp_config.json` |
| **腾讯 WorkBuddy** | `./install.sh --client workbuddy` | 原子合并到 `~/.workbuddy-ai/mcp.json`（或共用 `~/.codebuddy/.mcp.json`，带 `type: stdio`）；也可用 `--print workbuddy` 粘贴到桌面端「连接器 → 自定义连接器」 |
| **腾讯 CodeBuddy（CLI / IDE）** | `./install.sh --client codebuddy` | 原子合并到 `~/.codebuddy/.mcp.json`（`mcpServers.omp`，带 `type: stdio`） |
| **字节跳动 Trae / Trae CN** | `./install.sh --client trae` | 原子合并到 `~/.trae/mcp.json`（Trae CN 独立目录时写入 `~/.trae-cn/mcp.json`） |
| **阿里云 Qoder** | `./install.sh --client qoder` | 原子合并 `mcpServers.omp` 到 `~/.qoder/settings.json`（严格保留其他配置项） |
| **阿里云通义灵码** | `omp-toolkit setup --print lingma` | 输出标准 `mcpServers` JSON 供 IDE 侧边栏「个人设置 → MCP 服务 → 手动添加」粘贴 |
| **Claude Code** | `./install.sh --client claude-code` | 调用 `claude mcp add --scope user --transport stdio omp -- <bin> mcp`（不改 `CLAUDE.md`） |
| **ChatGPT 桌面端 / Codex** | `./install.sh --client codex`（或 `chatgpt`） | 保留注释原子更新 `~/.codex/config.toml` 的 `[mcp_servers.omp]`（不改 `AGENTS.md`，无 `codex` CLI 也能生效） |
| **Cursor** | `./install.sh --client cursor` | 原子合并到 `~/.cursor/mcp.json`（`mcpServers.omp`） |
| **VS Code / GitHub Copilot** | `./install.sh --client vscode` | 原子合并到用户级 `Code/User/mcp.json`（`servers.omp`） |
| **Claude Desktop** | `./install.sh --client claude-desktop` | 原子合并到 `claude_desktop_config.json`（`mcpServers.omp`） |
| **Windsurf** | `./install.sh --client windsurf` | 原子合并到 `~/.codeium/windsurf/mcp_config.json`（`mcpServers.omp`） |
| **AWS Kiro** | `./install.sh --client kiro` | 原子合并到 `~/.kiro/settings/mcp.json`（`mcpServers.omp`） |
| **Cline（VS Code 扩展）** | `./install.sh --client cline` | 原子合并到 `globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` |
| **Roo Code（VS Code 扩展）** | `./install.sh --client roo-code` | 原子合并到 `globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json` |
| **Zed** | `omp-toolkit setup --print zed` | 输出 `context_servers.omp` JSON 片段供粘贴到 JSONC 设置中 |

> 完整配置路径、macOS/Linux GUI 应用 `PATH` 继承处理与冲突保护细节见 [docs/clients.md](docs/clients.md)。

---

## 快速上手

构建需要 Rust 工具链（[rust-toolchain.toml](rust-toolchain.toml) 指定的 nightly 版本用于上游 `xutf` 的 `portable_simd`）。

### 1. 仅编译并安装二进制（不动任何 AI 软件配置）

```sh
./install.sh
# 单一可执行文件安装到 ~/.local/bin/omp-toolkit
```

### 2. 检测本机已安装的 AI 软件并演练预览（只读不写盘）

```sh
omp-toolkit setup --client list
omp-toolkit setup --client auto --dry-run
```

### 3. 一键编译、安装并适配

```sh
# macOS / Linux
./install.sh --client antigravity                                       # 仅适配反重力（推荐原生插件）
./install.sh --client antigravity --client claude-code --client cursor  # 同时适配多个客户端
./install.sh --client auto                                              # 自动适配所有已检测到的客户端
```

```powershell
# Windows PowerShell
.\install.ps1 -Client antigravity
.\install.ps1 -Client auto -DryRun
```

### 4. 一键无痕卸载（保留其他 MCP 配置）

```sh
omp-toolkit setup --client antigravity --remove
omp-toolkit setup --client cursor --remove
```

语言服务器与调试适配器按需自行安装到系统 `PATH`（如 `rust-analyzer`、`typescript-language-server`、`pyright`、`gopls`、`lldb-dap`、`dlv`、`debugpy`），工具会自动探测并常驻复用。

---

## CLI 常用示例

```sh
# 智能折叠阅读大文件（按 ~120 行预算渐进展开）或精准展开单个完整语法块
omp-toolkit summary src/code_ast.rs --unfold-until 120
omp-toolkit summary src/code_ast.rs --block-line 135

# 快速提取文件符号大纲
omp-toolkit outline src/mcp.rs --type function

# 全仓并行 AST 搜索（自动推断语言并遵守 .gitignore）
omp-toolkit ast-grep --pattern 'oldCall($$$ARGS)' --include 'src/**/*.ts'

# 全仓并行 AST 改写预览（确认后加 --apply 原子写盘）
omp-toolkit ast-edit --pattern 'oldCall($$$ARGS)' --rewrite 'newCall($$$ARGS)'

# 全仓扫描与结构化解决 Git 合并冲突（默认预览 diff，加 --apply 写盘）
omp-toolkit conflicts scan
omp-toolkit conflicts resolve src/app.rs --index 1 --choice ours

# 查看当前项目匹配的 LSP 服务器与本机 DAP 适配器
omp-toolkit lsp status
omp-toolkit debug-adapters

# 压缩冗长构建或失败测试输出
cargo test 2>&1 | omp-toolkit minimize --cmd "cargo test" --exit-code 101
```

---

## 项目文档导航

- [docs/clients.md](docs/clients.md) — 17 种国内外 AI 软件支持矩阵、一键适配/卸载与 GUI `PATH` 避坑指南
- [docs/tools.md](docs/tools.md) — 8 个 MCP 工具参数手册与完整 CLI 参考
- [docs/architecture.md](docs/architecture.md) — 精简抽取原则、架构图与核心工程设计
- [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) — 上游 Oh My Pi (`v19.1.0`) 与 RTK 开源归属声明

## 许可证

MIT — 详见 [LICENSE](LICENSE)。
