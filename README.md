# OMP Codex Toolkit

这套独立维护的工具把 Oh My Pi 的 AST 改写、Hashline 编辑、Shell、LSP 和调试能力接入 Codex 和终端。MCP 提供本地读取、诊断和补丁预览。`omp-refactor` 将补丁写到标准输出，诊断写到标准错误，补丁由使用者审阅后通过 Git 应用。

上游版本为 [v19.0.0](https://github.com/can1357/oh-my-pi/tree/v19.0.0)，提交为 `57771ae562b9345e0579bceb2d68d585e3e6927e`。本目录含上游 Rust 引擎及依赖源码，保留许可证。`src/` 中是独立命令的适配层。

## 安装

首版在 macOS 上验证。准备 Git、Rustup、Node.js 24 或更新版本、Xcode 命令行工具和 [Oh My Pi 19.0.0](https://github.com/can1357/oh-my-pi/tree/v19.0.0#install)。从源码构建会下载锁定的 Rust 依赖，并从固定的上游提交构建 Shell。选装通用语言服务器时还需 npm。

```sh
git clone https://github.com/y26s4824k264/omp-codex-toolkit.git
cd omp-codex-toolkit
./scripts/install.sh --with-lsp-servers
```

默认安装到 `$HOME/.local/omp-codex-toolkit`。可用 `--prefix` 指定目录，并用 `--omp-bin` 选择已安装的 Oh My Pi 19.0.0。同一目录重复安装会更新同名命令，并保留此前安装的可选语言服务器。安装完成后，按脚本输出将 `bin` 加入 `PATH`。需要接入 Codex 时，执行脚本输出的 `codex mcp add` 命令。已有同名 MCP 配置时，先核对配置指向的工具路径。

| 入口 | 功能 |
| --- | --- |
| `omp-codex-mcp` | 10 个 stdio MCP 工具，包括 AST、Hashline、日志、服务就绪和性能报告 |
| `omp-refactor` | 结构化搜索、补丁预览和源码摘要 |
| `omp-shell`、`obash` | Shell 执行、日志摘要和 Bash 语法解析 |
| `omp-lsp` | 多语言诊断、导航和语义重命名预览 |
| `omp-debug` | 基于 DAP 的任务级断点调试 |
| `codex-context-stats` | 读取指定会话日志的本地 token 用量统计 |

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

## 构建

工具链固定在上游的 `nightly-2026-11-08`。在本目录运行以下命令。

```sh
cargo build --release --locked -p omp-refactor
```

生成的二进制位于 `target/release/omp-refactor`，运行时需要 Git。

## 独立命令行

`omp-shell` 直接调用 OMP 19.0.0 的原生进程 Shell，支持 Bash、Zsh、POSIX sh 和原生退出码。`obash` 是同一源码中的独立测试与基准入口，可执行 Bash、只解析语法或记录运行轨迹。 [Rust 进程 Shell](https://github.com/can1357/oh-my-pi/blob/v19.0.0/crates/shell/src/process/mod.rs) · [obash 源码](https://github.com/can1357/oh-my-pi/blob/v19.0.0/crates/shell/src/bin/obash/main.rs)

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

构建时，在 v19.0.0 源码的 `crates/shell/examples/omp_shell_cli/` 下将 `shell/omp_shell_cli.rs` 放为 `main.rs`，将 `src/log_preview.rs` 放为 `log_preview.rs`，再在上游根目录构建。

```sh
cargo build --locked --profile ci -p omp-shell --example omp_shell_cli
cargo build --locked --profile ci -p omp-shell --bin obash
```

Shell 二进制位于 `target/ci/examples/omp_shell_cli`，安装时命名为 `omp-shell`。原版 `obash` 位于 `target/ci/obash`。

## 其他可独立使用的能力

- `omp toks FILE --json` 用本地 tokenizer 统计文件内容，可比较多种模型编码。Codex 首轮输入用量仍以实际请求日志为准。
- `omp read FILE:5-12` 和 `omp grep PATTERN PATH --limit 20` 可以直接处理本地文件。项目内的一般定位继续用 `rg`。
- OMP 的 `omp-diff` 和 `omp-xfs` 已作为编辑引擎依赖随源码抽取，可用于下一次需要 Rust 差异计算或文件扫描的工具。
- 代理调度、跨会话历史、会话压缩和 TUI 由 OMP 宿主管理。

## 接入 Codex

stdio MCP 启动命令为 `omp-codex-mcp`。在 Codex 的工具发现中搜索 `mcp__omp__`，打开对应工具 schema，按表中字段传入 JSON。`root` 为绝对项目目录，`file`、`files` 和 `log` 为目录内相对路径。

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
| `token_count` | 按若干本地 tokenizer 估算文件 token | `root`、`file`。实测 Codex 首轮用量用 `codex-context-stats` |

例如，要先盘点重复调用，选择 `ast_find` 并提交下列参数。

```json
{"root":"/absolute/project","files":["src/a.ts","src/b.ts"],"language":"typescript","pattern":"oldCall($VALUE)","limit":20}
```

结果过长时按返回的 `Full JSON` 路径读所需字段。需要改写时，`ast_preview` 和 `hashline_preview` 返回的 `Full patch` 指向完整补丁。审阅后在项目根先执行 `git apply --check -- "$patch"`，再执行 `git apply -- "$patch"`。源文件变化后重新预览。失败日志和超过 4 MiB 的日志会直接取原始头尾，其余日志使用 OMP 的命令过滤器。

终端用 `omp-refactor ast-find`、`ast`、`read-summary`、`read`、`hashline` 对应代码工具，用 `obash --json=native -c '…'` 解析 Bash，用 `omp-shell minimize` 摘要已有日志，用 `omp toks FILE --json` 估算文件 token。下方是 stdio MCP 的安装命令。

```sh
cargo build --release --locked -p omp-refactor --bin omp-codex-mcp
codex mcp add omp -- /absolute/path/to/omp-codex-mcp \
  --bin-dir /absolute/directory/with/native-tools --omp /absolute/path/to/omp \
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

`omp-lsp` 通过随命令安装的扩展直接调用已安装 OMP 19.0.0 的 LSP 引擎。`catalog` 列出 OMP 原生的全部 55 个服务器定义。`status` 显示当前项目的配置解析情况。项目根标记、项目本地二进制和项目级 LSP 配置按 OMP 规则解析。一次 `check` 可以检查多个明确列出的文件，并在本次调用内复用服务器。退出码 `0` 表示收到报告且无错误，`1` 表示报告中有错误，`2` 表示调用失败或报告不完整。

```sh
omp-lsp --json catalog | jq -r '.servers[].name'
omp-lsp --root /absolute/project status
omp-lsp --root /absolute/project check src/main.ts src/helper.ts
omp-lsp --root /absolute/project definition src/main.ts --line 12 --symbol load
```

`lsp/` 中的运行模块须放在同一目录。命令需要 Node 24 和 OMP 19.0.0。可在 `lsp/servers` 目录内运行 `npm ci --ignore-scripts` 安装锁定的通用服务器依赖。引擎优先解析项目本地依赖，再查看随 `omp-lsp` 安装的 `servers/node_modules/.bin` 和系统 PATH。JSON、CSS、SCSS 和 Less 验证默认开启，文件诊断跳过只提供补全的 Emmet，项目配置可覆盖这些默认值。Bash 的 lint 诊断需要 PATH 中的 ShellCheck。各服务器需要对应语言工具链和项目配置，冷启动可用 `--timeout` 延长。文件级 LSP 反馈用于辅助判断，项目级构建和测试仍按项目要求执行。

### 语义重命名预览

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

`omp-debug` 通过 OMP 19.0.0 的原生 DAP 调试器复用一个任务的断点、调用栈和变量状态。命令需要 Node 24 和目标语言的调试适配器。macOS C/C++ 可使用 Xcode 或 Command Line Tools 提供的 `lldb-dap`。

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

### Codex 上下文用量

`codex-context-stats` 用 Node 读取显式指定的 Codex 会话 JSONL。输出首个模型请求的输入和缓存 token、上下文占比，以及同一轮内后续请求的输入增长。`--turns` 展开轮次，`--requests` 展开逐请求用量，`--turn` 可指定 1 开始的轮次编号或轮次 ID。

```sh
codex-context-stats --turn 1 --requests /absolute/path/to/rollout.jsonl
```

## 许可

项目适配层使用 [MIT 许可证](LICENSE)。摘取的上游代码和第三方夹具沿用各自原许可证，详见 [第三方声明](THIRD_PARTY_NOTICES.md)。本仓库独立维护，与 Oh My Pi 和 Codex 项目无官方隶属关系。
