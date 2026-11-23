# OMP Toolkit

[English](README.md) · 简体中文

在 WorkBuddy、Codex、Claude 等编程智能体和终端中使用独立的 AST、Hashline、Shell、LSP 与调试工具。MCP 服务以本地 stdio 运行，提供源码读取、日志检查和补丁预览。`omp-refactor` 直接生成补丁，诊断写到标准错误。审阅后用 Git 应用补丁。

基础 Rust 编辑引擎和语言服务器默认配置来自 Oh My Pi v19.0.0（`57771ae562b9345e0579bceb2d68d585e3e6927e`）。Shell 和 `obash` 从 v19.1.0（`a7135b8f31826c878734388b648f52bed8da436e`）构建，工具也使用该版本中的部分文件系统与编辑修复。安装后的工具可独立运行，无需启动 Oh My Pi 或登录模型账户。 [v19.0.0 源码](https://github.com/can1357/oh-my-pi/tree/57771ae562b9345e0579bceb2d68d585e3e6927e) · [v19.1.0 源码](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e)

## 安装

从 [GitHub Releases](https://github.com/y26s4824k264/omp-toolkit/releases/latest) 下载并运行适合当前系统的脚本。安装器校验发行包的 SHA-256，复用兼容的 Node.js 与 npm 或下载并校验官方运行时，并安装锁定版本的常见语言服务器。检查和应用补丁、LSP 重命名预览时使用 Git。

下面的命令安装已发布的 v0.2.0。Magic 和 Sloppy 编辑预览、`learn` 子命令及新的 `omp-context-stats` 命令名，需要从当前分支构建。

也可以把下面这句话交给当前的编程智能体执行。

> 请按当前系统安装 OMP Toolkit，把它的本地 MCP 接到这个智能体，并确认 `ast_find` 可以调用。https://github.com/y26s4824k264/omp-toolkit

**macOS 和 Linux**

```sh
curl -fsSLo install.sh https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.2.0/install.sh && bash install.sh --version v0.2.0
```

默认安装到 `$HOME/.local/omp-toolkit`。可用 `--prefix DIR` 指定目录，或用 `--without-lsp-servers` 跳过常见语言服务器的安装。

**Windows PowerShell 5.1 或更新版本**

```powershell
Invoke-WebRequest -UseBasicParsing https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.2.0/install.ps1 -OutFile install.ps1
& .\install.ps1 -Version v0.2.0
```

默认安装到 `%LOCALAPPDATA%\OMP Toolkit`。对应参数包括 `-Prefix DIR` 和 `-WithoutLspServers`。若机器策略限制运行下载的 PowerShell 脚本，请按本机或组织的流程审核并允许运行。

这些命令使用带版本号的下载地址，GitHub 匿名 API 限流时也能安装。要切换版本，请同时改下载地址和脚本参数里的版本号。

安装器会检测本机可用的 Codex 和 Claude Code 命令并接入 MCP。客户端的 `omp` 条目指向本次安装时，安装器也会把带标记的工具优先规则追加到全局指令。仅需工具和配置文件时，macOS/Linux 用 `--client none`，Windows 用 `-Client none`。按脚本输出可将 `bin` 目录加入当前终端的 `PATH`。

新版本的默认安装目录与 `v0.1.0` 源码版分开。切换到新安装时，将客户端中旧的 `omp` 条目换成新生成的条目。安装器无法自动接入时会打印可在终端执行的命令。

如果手动配置的 MCP 条目指向早期可执行文件别名，原地升级前请把启动命令切到 `omp-toolkit-mcp`。

### 系统与功能

| 系统 | 发行包架构 | 系统要求 |
| --- | --- | --- |
| macOS | x64、arm64 | macOS 15+ |
| Linux | x64、arm64 | 使用 glibc 的发行版，如 Ubuntu 22.04+ |
| Windows | x64、arm64 | PowerShell 5.1+ |

LSP 符号和文件重命名预览需要 macOS。`omp-debug` 持久 DAP 会话在 macOS 和 Linux 上需要对应语言的调试适配器。Windows 暂不提供持久 DAP 会话。V8 性能报告可在三种系统上读取，`sample` 报告由 macOS 采集。智能体客户端有自己的系统要求。

macOS 预编译包使用系统自带的 `open` 打开文件和网址。

| 入口 | 功能 |
| --- | --- |
| `omp-toolkit-mcp` | 10 个 stdio MCP 工具，包括 AST、Hashline、日志、服务就绪和性能报告 |
| `omp-refactor` | 结构化搜索、补丁预览、源码摘要和经验积累 |
| `omp-magic` | 为智能体准备描述式编辑，并预览它返回的行操作 |
| `omp-shell`、`obash` | Shell 执行、日志摘要和 Bash 语法解析 |
| `omp-lsp` | 多语言诊断、导航和语义重命名预览 |
| `omp-debug` | 基于 DAP 的任务级断点调试 |
| `omp-context-stats` | 通过 Codex 会话日志统计实际 token 用量 |

## 接入编程智能体

安装器在安装目录中生成 `mcp-config.json` 和 `vscode-mcp.json`，文件中的启动命令和参数已填入本机绝对路径。手动配置时只把 `omp` 条目合并到对应的服务器对象。运行在云端的会话需要能访问运行该工具的执行环境。

客户端需要手动设置工具偏好时，可把安装目录里的 `AGENT-INSTRUCTIONS.md` 复制到其全局指令。

| 智能体 | 接入步骤 |
| --- | --- |
| WorkBuddy | 在「连接器 → 自定义连接器 → 添加 MCP」中，把 `mcp-config.json` 的 `mcpServers.omp` 合并到 `mcp.json`。保存后若提示信任，请在客户端确认。个人版 Linux 入口见官网。 [配置](https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Connector) · [下载](https://www.workbuddy.cn/work/) |
| Codex | 安装器检测本机 CLI 并接入。用 `codex mcp list` 检查配置。 [官方文档](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) |
| Claude Code | 安装器检测本机 CLI 并接入。用 `claude mcp get omp` 检查配置。 [官方文档](https://code.claude.com/docs/en/mcp#option-3-add-a-local-stdio-server) |
| Claude Desktop | 在设置的 Developer → Edit Config 中合并 `mcp-config.json` 的 `mcpServers.omp`，保存后完全退出并重启。 [配置和路径](https://modelcontextprotocol.io/docs/develop/connect-local-servers) |
| Cursor、其他本地 MCP 客户端 | Cursor 可将 `mcpServers.omp` 合并到 `~/.cursor/mcp.json`。其他支持本地 stdio 的客户端使用生成配置中的 `command` 和 `args`。 [Cursor 文档](https://prod.cursor.com/help/customization/mcp) |
| VS Code 和 Copilot | VS Code 编辑器管理的 MCP 配置使用 `vscode-mcp.json` 中的 `servers.omp`。当前 Copilot 可在「Chat: Open Customizations → MCP Servers」添加，或者从 `mcp-config.json` 合并 `mcpServers.omp` 并在该条目中加 `"tools": ["*"]`。 [配置说明](https://code.visualstudio.com/docs/agents/reference/mcp-configuration) |

在新会话让智能体对测试项目调用 `ast_find`，或对测试文件调用 `hashline_read`。补丁预览会返回完整补丁，先审阅并运行 `git apply --check`，再应用。

## 结构化批量编辑

在项目根目录列出目标文件。先查看命中与补丁，再检查并应用。预览后目标文件有变化时重新生成补丁。

```sh
omp-refactor ast-find --lang typescript --pattern 'oldCall($VALUE)' \
  --limit 20 src/a.ts src/b.ts
patch_file=$(mktemp)
omp-refactor ast --lang typescript --pattern 'oldCall($VALUE)' \
  --rewrite 'newCall($VALUE, opts)' src/a.ts src/b.ts > "$patch_file"
cat "$patch_file"
git apply --check -- "$patch_file"
git apply -- "$patch_file"
git diff --check
git diff -- src/a.ts src/b.ts
```

AST 按语法结构匹配。符号归属由语言服务器核对。Go 项目中可先执行 `gopls rename -d file.go:行:列 NewName` 预览，再用 `-w` 写入。抽取版包含 OMP 的内置语法，支持 Go、TypeScript、Rust 等语言。

`ast-find` 提供只读搜索，返回命中原文、文件位置、捕获值和分页信息。用 `--offset` 读取后续页，结果按输入文件和文件内的位置排序。

## Hashline 编辑

先用 `omp-refactor read src/a.ts --start 10 --end 18` 读取文件。把输出中的 `[路径#标签]` 复制到补丁中。下面的标签和行号是示例。

面对大文件时，可用 `omp-refactor read-summary src/a.ts` 先查看声明和源文件行号，再读修改范围以获得新标签。

```text
*** Begin Patch
[src/a.ts#A1B2]
PUT 12.=12:
+const value = calculate();
*** End Patch
```

`omp-refactor hashline --patch edits.txt src/a.ts` 生成同样的 Git 补丁。该入口处理既有普通文本文件，使用当前文件标签验证位置。文件变化后，重新读取目标行并生成补丁。文件创建、移动和删除使用项目现有编辑工具。

## Sloppy 和 Magic 编辑预览

Sloppy 使用 OMP 原生的锚点匹配，支持文本改动和按语法定位的代码块操作。调用时列出补丁中的每个文件。任一操作失败会拒绝整批改动。块选择器、剪切和粘贴见 [Sloppy 语法](https://github.com/can1357/oh-my-pi/blob/v19.1.0/crates/edit/prompts/sloppy.md)。

```sh
printf '%s\n' '*** Edit File: src/a.ts' '*** Find' 'oldCall(7)' '*** Replace' 'newCall(7)' > change.edit
omp-refactor sloppy --patch change.edit src/a.ts > change.patch
git apply --check -- change.patch
```

Magic 按 `# 相对路径` 分组描述改动。`prepare` 为每个文件生成带行号的源码和快照摘要。把计划里的 `systemPrompt` 和每个文件的 `request` 交给编程智能体，再把它给出的官方 Magic 行操作保存到 `replies.json`。`preview` 核对原始快照和全部操作，生成可审阅的 Git 补丁。如果摘录省略了需要改动的行，可以用 `prepare --full` 重新生成计划。

```sh
printf '%s\n' '# src/a.ts' '- Replace oldCall with newCall on line 1.' > changes.md
omp-magic prepare --root . --instructions changes.md > plan.json
```

```json
{"replies":[{"path":"src/a.ts","operations":"[\"sub\",1,\"oldCall\",\"newCall\"]"}]}
```

```sh
omp-magic preview --root . --plan plan.json --replies replies.json > change.patch
git apply --check -- change.patch
```

两种预览都针对已有文本文件，保留统一的 LF 或 CRLF 换行。模型调用和是否应用补丁由当前智能体处理。

## 从源码构建

macOS 和 Linux 源码安装需要 Git、Rustup、系统编译工具和 Node.js 24+。使用源码安装器装常见语言服务器时还需要 npm。

```sh
git clone https://github.com/y26s4824k264/omp-toolkit.git
cd omp-toolkit
./scripts/install.sh --with-lsp-servers
```

工具链固定在上游的 `nightly-2026-11-08`。在本目录运行以下命令。

```sh
cargo build --release --locked -p omp-refactor
```

生成的二进制位于 `target/release/omp-refactor`，运行时需要 Git。

## 独立命令行

`omp-shell` 直接调用 Oh My Pi v19.1.0 的原生进程 Shell，支持 Bash、Zsh、POSIX sh 和原生退出码。`obash` 是同一源码中的独立测试与基准入口，可执行 Bash、只解析语法或记录运行轨迹。 [Rust 进程 Shell](https://github.com/can1357/oh-my-pi/blob/v19.1.0/crates/shell/src/process/mod.rs) · [obash 源码](https://github.com/can1357/oh-my-pi/blob/v19.1.0/crates/shell/src/bin/obash/main.rs)

```sh
omp-shell bash -c 'printf "%s\n" alpha beta | grep alpha'
omp-shell zsh -f -c 'print -r -- "$ZSH_VERSION"'
obash --json=native -c 'printf "%s\n" alpha | grep alpha'
```

长命令可用 `run-log` 一次执行，合并保存原始 stdout 和 stderr，返回短预览与原始退出码。不传 `--log` 时创建私有临时文件，启动前打印路径。`--log` 指定的新文件也使用私有权限。传入的命令和参数会原样执行。管道、重定向等 shell 语法可以显式交给 `bash -c` 或 `zsh -c`。

```sh
omp-shell run-log -- go test ./...
omp-shell run-log --log ./test-output.log -- npm test
```

已有日志可单独用 `minimize` 处理。它报告退出码、前后字节数和原始日志位置。

```sh
log_file=$(mktemp)
test_status=0
go test ./... > "$log_file" 2>&1 || test_status=$?
omp-shell minimize "$log_file" "$test_status" 'go test ./...'
```

构建时，在 v19.1.0 源码的 `crates/shell/examples/omp_shell_cli/` 下将 `shell/omp_shell_cli.rs` 放为 `main.rs`，将 `src/log_preview.rs` 放为 `log_preview.rs`，再在上游根目录构建。

```sh
cargo build --locked --profile ci -p omp-shell --example omp_shell_cli
cargo build --locked --profile ci -p omp-shell --bin obash
```

Shell 二进制位于 `target/ci/examples/omp_shell_cli`，安装时命名为 `omp-shell`。原版 `obash` 位于 `target/ci/obash`。

用 `omp-shell process-tree PID --depth 2 --json` 查看长任务和子进程。输出包含进程编号和状态，省略命令行参数。

## 其他可独立使用的能力

- `omp-refactor ranked-find 'request timeout retry' --limit 8` 按关键词覆盖和稀有程度给文件排序，返回路径和命中行。精确查询继续用 `rg`。
- `omp-refactor token-count FILE` 用 `o200k_base`、`cl100k_base` 和四种 Claude 编码离线估算文件 token。Codex 实际输入用量以会话日志为准。
- `omp read FILE:5-12` 和 `omp grep PATTERN PATH --limit 20` 可以直接处理本地文件。项目内的一般定位继续用 `rg`。
- OMP 的 `omp-diff` 和 `omp-xfs` 已作为编辑引擎依赖随源码抽取，可用于下一次需要 Rust 差异计算或文件扫描的工具。

## MCP 工具

stdio MCP 启动命令为 `omp-toolkit-mcp`。在 Codex 的工具发现中搜索 `mcp__omp__`，其他客户端按 `omp` 查找。`root` 为绝对项目目录，`file`、`files` 和 `log` 为目录内相对路径。

| MCP 工具 | 用途 | JSON 字段 |
| --- | --- | --- |
| `ast_find` | 按语法结构查命中、位置和捕获值 | `root`、`files`、`language`、`pattern`。可选 `limit` 和 `offset` 分页 |
| `ast_preview` | 预览结构性替换并生成完整 Git 补丁 | `root`、`files`、`language`、`pattern`、`replacement` |
| `read_summary` | 读取源码摘要和原文件行号 | `root`、`file` |
| `hashline_read` | 取真实源码行和当前标签，每次最多 120 行 | `root`、`file`、`start`、`end`。从第 1 行开始编号，含两端 |
| `hashline_preview` | 使用新鲜标签预览 `PUT`、`CUT` 锚点编辑 | `root`、`files`、`patch`。文件列表与补丁头对应 |
| `shell_parse` | 只读解析 Bash 语法树 | `script` 为 Bash 源码 |
| `log_minimize` | 从已有日志生成短摘要，保留原始路径和退出码 | `root`、`file`、`command`、`exit_code` |
| `service_ready` | 等待本机端口或启动日志，可检测进程退出 | `root`。传 `port`、日志三件套 `log`/`log_pattern`/`log_offset` 或两者。可选 `pid`、`timeout_ms` |
| `profile_summary` | 摘要 V8 CPU profile 或 macOS `sample` | `root`、`file` |
| `token_count` | 按若干本地 tokenizer 估算文件 token | `root`、`file`。用 `omp-context-stats` 读取 Codex 会话日志中的实际用量 |

例如，要先盘点重复调用，选择 `ast_find` 并提交下列参数。

```json
{"root":"/absolute/project","files":["src/a.ts","src/b.ts"],"language":"typescript","pattern":"oldCall($VALUE)","limit":20}
```

结果过长时按返回的 `Full JSON` 路径读所需字段。需要改写时，`ast_preview` 和 `hashline_preview` 返回的 `Full patch` 指向完整补丁。审阅后在项目根先执行 `git apply --check -- "$patch"`，再执行 `git apply -- "$patch"`。源文件变化后重新预览。失败日志和超过 4 MiB 的日志会直接取原始头尾，其余日志使用 OMP 的命令过滤器。

终端用 `omp-refactor ast-find`、`ast`、`read-summary`、`read`、`hashline` 和 `token-count`，用 `obash --json=native -c '…'` 解析 Bash，用 `omp-shell minimize` 摘要已有日志。手动从源码构建后，可用下面的命令接入 Codex。

```sh
cargo build --release --locked -p omp-refactor --bin omp-toolkit-mcp
codex mcp add omp -- /absolute/path/to/omp-toolkit-mcp \
  --bin-dir /absolute/directory/with/native-tools \
  --node /absolute/path/to/node --profile-helper /absolute/path/to/profile/summary.mjs
```

连续 Shell 步骤在 Codex 的任务专用 PTY 里运行 `exec omp-shell zsh -f -i`，后续输入发给同一会话。该进程保留当前目录、变量和函数，关键命令可以读回 `$?`。完成后执行 `exit`。

### 本地服务验收

启动前记录准备重用的日志长度，或者为该次启动新建日志并令偏移为 0。启动进程后，调用 `service_ready`，传入实际配置的本机端口和日志条件。检测只匹配该偏移之后的完整新行，两项条件都提供时须同时满足。可传入任务 PID 判断提前退出。端口检测确认本机存在监听，需要验证应用行为时另做 HTTP 健康检查。

```json
{"root":"/absolute/project","port":5173,"log":"work/dev.log","log_pattern":"ready in","log_offset":0,"timeout_ms":10000}
```

### 性能报告

`profile_summary` 读取项目下已有的 V8 `.cpuprofile` 或 macOS `.sample.txt`。结果列出热点、关键调用树和线程信息，输入文件上限为 32 MiB。安装时复制整个 `profile/` 目录，保留适配器旁边的三个 OMP 源文件，并使用支持 TypeScript 类型擦除的 Node 24。

### 多语言诊断与导航

`omp-lsp` 直接启动语言服务器并走 LSP stdio。`catalog` 列出抽取的 55 个服务器定义，`status` 显示当前项目配置。一次 `check` 可以检查多个明确列出的文件并复用服务器。退出码 `0` 表示收到报告且无错误，`1` 表示报告中有错误，`2` 表示调用失败或报告不完整。

```sh
omp-lsp --json catalog | jq -r '.servers[].name'
omp-lsp --root /absolute/project status
omp-lsp --root /absolute/project check src/main.ts src/helper.ts
omp-lsp --root /absolute/project definition src/main.ts --line 12 --symbol load
```

`lsp/` 运行模块放在一起，使用 Node.js 24+。手工安装常见语言服务器时，可在 `lsp/servers` 目录运行 `npm ci --ignore-scripts`。工具依次查询项目依赖、已安装的服务器和 `PATH`。Bash lint 诊断使用可用的 ShellCheck，Go 和 Rust 使用项目选定的工具链。冷启动可用 `--timeout` 延长，构建和测试仍按项目要求执行。

### 语义重命名预览

项目语言服务器配置依次读取 `.omp/lsp.json`、`.lsp.json` 和 `lsp.json`，后者覆盖前者。原先存于 Oh My Pi 用户配置或 YAML 的设置可移入项目 JSON。

`rename-preview` 让项目语言服务器定位符号的定义和实际引用。`rename-file-preview` 预览文件移动及语言服务器给出的导入更新。目标只含文件名时沿用源文件目录，多级路径相对于项目根。两项在 macOS 的 Git 工作树中生成完整 Git 补丁，默认写到私有临时文件，也可用 `--patch` 指定新文件。`--json` 返回补丁路径、预览和 Git 应用目录 `apply_root`。支持范围取决于项目语言服务器提供的重命名能力。

```sh
patch_dir=$(mktemp -d)
omp-lsp --root /absolute/project rename-preview src/config.ts \
  --line 12 --symbol oldName --new-name newName --patch "$patch_dir/rename.patch"
git -C /absolute/project apply --check -- "$patch_dir/rename.patch"

omp-lsp --root /absolute/project rename-file-preview src/config.ts --new-name settings.ts
```

以返回路径中的完整补丁作为审阅对象，在返回的 Git 应用目录中执行 `git apply --check`。`--root` 可以选子项目，补丁中的路径仍从 Git 仓库根开始。源码发生变化后重新生成预览。

### 多轮断点调试

`omp-debug` 与适配器直接走 DAP，保留一个任务的断点、调用栈和变量状态。macOS 和 Linux 持久会话需要 Node.js 24 和目标语言的调试适配器。macOS C/C++ 可使用 Xcode 或 Command Line Tools 提供的 `lldb-dap`。

将 `debug/omp-debug.mjs`、`debug/bridge.mjs` 和 `debug/protocol.mjs` 安装在同一目录，再将 `omp-debug.mjs` 以 `omp-debug` 的名称链接到 PATH 中。

在任务专用 PTY 或 Codex Process Jobs 中运行前台服务。`--session` 用于指定该任务尚未使用的目录，其绝对路径需要传给后续每条命令。

```sh
debug_dir=$(mktemp -d)
session="$debug_dir/session"
omp-debug --session "$session" serve --root /absolute/project
```

看到 `OMP_DEBUG_READY` 后，在另一个终端用同一个 `session` 路径发请求。为可执行文件保留调试符号。栈帧 ID 和变量引用从上一条命令的 JSON 中取。运行中的程序可用 `pause` 暂停，`stop` 结束任务。默认 15 分钟空闲后退出，可用 `serve --idle 秒数` 调整。

```sh
omp-debug --session "$session" launch ./build/app --adapter lldb-dap
omp-debug --session "$session" break src/main.c 12
omp-debug --session "$session" continue
omp-debug --session "$session" stack
omp-debug --session "$session" scopes "$frame_id"
omp-debug --session "$session" variables "$variables_ref"
omp-debug --session "$session" step over
omp-debug --session "$session" stop
```

### 经验积累与技能草稿

可以保存编程智能体总结的可复用经验，也可以把审阅过的 Markdown 流程整理成可移植的 `SKILL.md` 草稿。工具使用项目内的 `.omp-toolkit/learning`。`preview` 在终端展示草稿，`create` 写入新草稿，`update` 显式替换已有草稿。审阅后可把整个技能目录复制到所用客户端的技能目录。

```sh
omp-refactor --root . learn save 'Check the complete Git patch before applying it.'
omp-refactor --root . learn list
omp-refactor --root . learn skill preview patch-review --description 'Review a Git patch before applying it' --body-file ./patch-review.md
```

用 `--body-file -` 可从标准输入读取 Markdown 正文。要积累的内容由当前编程智能体决定，工具在本地处理传入的文本。

### 上下文用量

`omp-context-stats` 用 Node 读取显式指定的会话日志，目前支持 Codex rollout JSONL 格式。输出首个模型请求的输入和缓存 token、上下文占比，以及同一轮内后续请求的输入增长。`--turns` 展开轮次，`--requests` 展开逐请求用量，`--turn` 可指定 1 开始的轮次编号或轮次 ID。

```sh
omp-context-stats --turn 1 --requests /absolute/path/to/rollout.jsonl
```

## 许可

项目适配层使用 [MIT 许可证](LICENSE)。摘取的上游代码和第三方夹具沿用各自原许可证，详见 [第三方声明](THIRD_PARTY_NOTICES.md)。本仓库独立维护，与 Oh My Pi 和 Codex 项目无官方隶属关系。
