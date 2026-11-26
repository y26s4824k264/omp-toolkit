# OMP Toolkit

[English](README.md) · 简体中文

在 WorkBuddy、Codex、Claude 或终端中使用 Oh My Pi 的源码检查、补丁预览、Shell、LSP 诊断和调试能力。工具在本机运行，无需另装 Oh My Pi 或登录模型账户。

## 安装 v0.3.0

安装器按 SHA-256 校验二进制包。`PATH` 中有兼容的 Node.js 和 npm 时会复用，否则从上游下载并校验。默认安装常用语言服务器。检查和应用补丁、预览 LSP 重命名时需安装 Git。 [发布包和校验值](https://github.com/y26s4824k264/omp-toolkit/releases/tag/v0.3.0)

**macOS 和 Linux**

```sh
curl -fsSLo install.sh https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.3.0/install.sh && bash install.sh --version v0.3.0
```

**Windows PowerShell 5.1 或更新版本**

```powershell
Invoke-WebRequest -UseBasicParsing https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.3.0/install.ps1 -OutFile install.ps1; if ($?) { & .\install.ps1 -Version v0.3.0 }
```

默认目录为 `$HOME/.local/omp-toolkit` 和 `%LOCALAPPDATA%\OMP Toolkit`。用 `--prefix DIR` 或 `-Prefix DIR` 自定义目录，用 `--without-lsp-servers` 或 `-WithoutLspServers` 跳过常用语言服务器。按安装输出将 `bin` 加入 `PATH`，再运行 `omp-refactor --version` 和 `omp-toolkit-mcp --version`。

如果只安装命令和生成接入配置，运行下载后的脚本并指定 `none`。

```sh
bash install.sh --version v0.3.0 --client none
```

```powershell
& .\install.ps1 -Version v0.3.0 -Client none
```

| 系统 | 架构 | 要求 |
| --- | --- | --- |
| macOS | x64、arm64 | macOS 15+ |
| Linux | x64、arm64 | 使用 glibc 的发行版，如 Ubuntu 22.04+ |
| Windows | x64、arm64 | PowerShell 5.1+ |

## 接入编程智能体

安装器默认检测 `PATH` 中的 Codex 和 Claude Code CLI，并以 `omp` 为名称注册本机 stdio 服务。macOS/Linux 上可用 `--client codex` 或 `--client claude` 单独选择，Windows 上用 `-Client codex` 或 `-Client claude`。已有的 `omp` 条目如果指向别处会保留。客户端配置指向本次安装时，安装器会在该客户端的全局指令中添加一段带标记的工具说明。

手动接入所需的 `mcp-config.json`、`vscode-mcp.json` 和 `AGENT-INSTRUCTIONS.md` 都在安装目录下。把生成的 `omp` 条目合并到客户端现有的服务列表即可，启动命令和参数已填好本机绝对路径。

| 客户端 | 接入与验证 |
| --- | --- |
| Codex | 运行 `codex mcp list` 检查是否已注册。 [文档](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) |
| Claude Code | 运行 `claude mcp get omp` 检查是否已注册。 [文档](https://code.claude.com/docs/en/mcp#option-3-add-a-local-stdio-server) |
| WorkBuddy | 在「连接器 → 自定义连接器 → 添加 MCP」中把 `mcpServers.omp` 合并到 `mcp.json`。如有信任提示，在 WorkBuddy 中确认。 [文档](https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Connector) |
| Claude Desktop | 在「Developer → Edit Config」中合并 `mcpServers.omp`，然后重启应用。 [文档和配置路径](https://modelcontextprotocol.io/docs/develop/connect-local-servers) |
| 其他 stdio 客户端 | 使用生成的 `command` 和 `args`。Cursor 和当前 Copilot 使用 `mcpServers.omp`，Copilot 条目加 `"tools": ["*"]`。由 VS Code 编辑器管理的配置使用 `servers.omp`。 [Cursor](https://prod.cursor.com/help/customization/mcp) · [VS Code 和 Copilot](https://code.visualstudio.com/docs/agents/reference/mcp-configuration) |

新开一个会话，让智能体在测试项目中调用 `ast_find`。Codex 中的工具前缀是 `mcp__omp__`。远程会话需要能访问装有工具的机器或执行环境。

## 检查源码和预览改动

从 Git 项目的根目录运行以下示例。编辑示例假定 `src/a.ts` 的第 1 行包含 `oldCall(7)`。用 `--root` 可选择另一个项目。调用 MCP 时，`root` 传项目绝对路径，文件字段传项目内相对路径。

**AST** 按语法结构匹配，可预览同一种改写在多个文件中的结果。

```sh
omp-refactor ast-find --lang typescript --pattern 'oldCall($VALUE)' src/a.ts
omp-refactor ast --lang typescript --pattern 'oldCall($VALUE)' --rewrite 'newCall($VALUE)' src/a.ts > ast.patch
git apply --check -- ast.patch
```

**Hashline** 会用文件当前标签校验补丁。先用 `omp-refactor read-summary` 定位代码，再用 `omp-refactor read` 读取最新行锚点。**Sloppy** 支持文本锚点和基于语法的块操作。命令需列出编辑文件中的所有目标文件。 [Sloppy 语法](https://github.com/can1357/oh-my-pi/blob/v19.1.0/crates/edit/prompts/sloppy.md)

```sh
printf '%s\n' '*** Edit File: src/a.ts' '*** Find' 'oldCall(7)' '*** Replace' 'newCall(7)' > change.edit
omp-refactor sloppy --patch change.edit src/a.ts > sloppy.patch
git apply --check -- sloppy.patch
```

**Magic** 把改动描述整理成供编程智能体读取的计划。每个文件的描述以 `# relative/path` 开头。把计划的 `systemPrompt` 和各文件的 `request` 交给智能体，内容可能包含项目源码。将返回的行操作保存到 `replies.json`，再用 `preview` 按原始快照校验并生成补丁。摘录缺少必要上下文时可用 `prepare --full`。 [Magic 源码说明](magic/UPSTREAM.md)

```sh
printf '%s\n' '# src/a.ts' '- Replace oldCall with newCall on line 1.' > changes.md
omp-magic prepare --root . --instructions changes.md > plan.json
```

```json
{"replies":[{"path":"src/a.ts","operations":"[\"sub\",1,\"oldCall\",\"newCall\"]"}]}
```

```sh
omp-magic preview --root . --plan plan.json --replies replies.json > magic.patch
git apply --check -- magic.patch
```

这些命令只预览，不直接写入源码。确认完整差异后，再用 `git apply -- PATH.patch` 应用。源码变化后需重新预览。

从当前源码构建的版本支持用 `sub` 插入多行替换文本。编辑后的文件解析失败时，预览会尝试指出破坏语法的操作，按提示修正后即可重跑。

## 保存项目经验和技能草稿

用 `--root` 指定记录所属的项目。`save` 去重保存经验，`list` 查看记录，`skill preview` 输出草稿。`create` 在 `.omp-toolkit/learning/skills/` 下创建新的 `SKILL.md`，`update` 用来更新已经创建的草稿。审阅后再安装到编程智能体中。

```sh
omp-refactor --root . learn save 'Check the complete Git patch before applying it.'
omp-refactor --root . learn list
printf '%s\n' 'Review the diff, then run git apply --check.' > patch-review.md
omp-refactor --root . learn skill preview patch-review --description 'Review a Git patch' --body-file patch-review.md
omp-refactor --root . learn skill create patch-review --description 'Review a Git patch' --body-file patch-review.md
omp-refactor --root . learn skill update patch-review --description 'Review a Git patch' --body-file patch-review.md
```

## 跟踪长期目标

当前源码版提供 `omp-refactor goal`。它把目标 ID、总目标、验收条件和当前里程碑存到项目的 `.omp-toolkit/goals/`。续做时用同一个项目根目录和 ID。用 Codex 等智能体的原生目标功能维持长期任务，用这份项目记录跟踪里程碑和验收证据。这个结构借鉴了 Oh My Pi 分开保存目标和待办的做法。 [Oh My Pi 目标续做](https://github.com/can1357/oh-my-pi/blob/v19.1.1/packages/coding-agent/src/prompts/goals/goal-continuation.md) · [待办状态](https://github.com/can1357/oh-my-pi/blob/v19.1.1/packages/coding-agent/src/prompts/goals/goal-todo-context.md)

```sh
omp-refactor --root . goal init release --objective 'Publish and verify the update' --criterion 'Platform tests pass' --criterion 'Public installation works'
omp-refactor --root . goal focus release 'Run the platform tests'
omp-refactor --root . goal record release 1 'CI run 123 passed the required platform jobs'
omp-refactor --root . goal show release
```

`goal list` 列出已有 ID。`goal clear-evidence release 1` 可撤回失效证据。`ready_for_review` 表示每项条件都已登记证据，关闭原生目标前还需核对这些证据是否仍能证明完成。这个命令需要当前源码版，v0.3.0 发行包早于这次改动。

## 其他命令

| 用途 | 命令 |
| --- | --- |
| 按多个关键词定位文件 | `omp-refactor ranked-find "keywords" --limit 10` |
| 运行命令并保留完整日志 | `omp-shell run-log -- COMMAND ARGS...`。用 `omp-shell minimize` 摘要已有日志，用 `obash --json=native -c '…'` 解析 Bash 语法。 |
| 查看语言服务器和诊断 | `omp-lsp catalog`、`omp-lsp status`、`omp-lsp check FILE...` 或 `omp-lsp definition`。诊断依赖项目的语言服务器。符号和文件重命名预览需要 macOS。 |
| 使用多轮调试会话 | `omp-debug --session ABSOLUTE_DIR serve --root PROJECT`，再从另一个终端发送 `launch`、`break`、`continue` 或 `stop`。macOS 和 Linux 需要对应的 DAP 适配器。Windows 暂无持久 DAP 会话。 |
| 统计日志中记录的上下文用量 | `omp-context-stats --turn 1 --requests ROLLOUT.jsonl` 读取 Codex rollout JSONL。 |
| 通过 MCP 检查本机运行状态 | `service_ready` 检查环回端口或新增日志行。`profile_summary` 读取 V8 或 macOS `sample` 报告。`token_count` 离线估算文件 token。 |

用命令或子命令的 `--help` 查看参数。MCP 客户端会显示各工具的参数结构。

从当前源码构建的 `ranked-find` 会遵守 Git worktree 共享的 `info/exclude`。用该 worktree 作为 `--root`，搜索超时后缩小范围再查。macOS 和 Linux 上取消前台任务时，向本次 `run-log` 发送 `SIGINT` 或 `SIGTERM`，它会清理所属进程并保留原始日志。需要持续运行的服务交由宿主的任务管理器。

## 通过 MCP 读取 Office 和图片

两个工具都接收绝对目录 `root` 和目录内的相对路径 `file`。

| 用途 | 除 `root` 和 `file` 之外的参数 |
| --- | --- |
| 读取 DOCX 正文 | `office_read` 接收 `start`、`end`。从 1 开始，包含首尾，每个段落或表格行占一个块。 |
| 读取 PPT 文字和演讲者备注 | `office_read` 接收 `start`、`end`。按放映顺序从 1 开始，包含首尾。 |
| 读取 Excel | 省略 `sheet` 可列出工作表。指定 `sheet` 和 A1 `range`，例如 `B4:E12`，可返回非空单元格的坐标、值和公式。公式单元格的值来自保存的缓存，缺少缓存时为 `null`。 |
| 按需看图 | `image_read` 默认返回尺寸、格式和文件大小。指定 `max_edge` 可返回预览，或用 `crop: { x, y, width, height }` 查看局部。坐标从 0 开始，按应用 EXIF 方向后的原图像素计算。裁剪预览默认长边上限为 768 像素。 |

Office 默认每次返回最多 8,000 字节的内部 JSON。后续使用返回的 `next_offset` 作为 `offset`，并沿用原范围和字节预算。长记录可按 `part` 拼接 `record_json`，到 `last: true` 再解析。旧版 XLS、PPT 需要 LibreOffice（`soffice`），转换在临时副本上执行。转换可能规范化公式语法、重新计算缓存值。检查版式、图表和嵌入图片时使用视觉渲染。

## 从旧版升级

旧脚本中的 `codex-context-stats` 要替换为 `omp-context-stats`。手写 MCP 配置应指向 `omp-toolkit-mcp`，Windows 上是 `omp-toolkit-mcp.exe`。新版安装包已移除旧的 `omp-codex-mcp` 别名。生成的配置可用来核对启动路径和参数。

## 源码和许可证

macOS 或 Linux 从源码安装时需要 Git、Rustup、系统编译工具和 Node.js 24+。安装常用语言服务器还需 npm。在源码目录运行 `./scripts/install.sh --with-lsp-servers`，切到对应的 Git 标签可构建指定版本。 [源码安装脚本](scripts/install.sh) · [发布工作流](.github/workflows/release.yml)

本项目独立维护，采用 MIT 许可证。基础编辑引擎与 LSP 默认配置来自 Oh My Pi v19.0.0。Shell 和 `obash` 从 Oh My Pi v19.1.0 构建，也采用了该版本的部分文件系统和编辑修复。各平台压缩包内包含依赖声明，发布页提供校验值。 [许可证](LICENSE) · [第三方声明与确切上游版本](THIRD_PARTY_NOTICES.md)
