## Install / 安装

Prebuilt packages for macOS, Linux and Windows are available in Assets. The setup scripts verify package checksums and create local MCP configuration for WorkBuddy, Codex, Claude Code, Claude Desktop and other stdio clients.

Assets 提供 macOS、Linux 和 Windows 安装包。安装脚本校验包的哈希，并为 WorkBuddy、Codex、Claude Code、Claude Desktop 等支持 stdio 的客户端生成本地配置。

Language servers, debug adapters, and offline tokenizers run independently of the Oh My Pi application. AST and Hashline previews produce unified patches in process. Use ordinary `git apply --check` and `git apply` for those patches.

语言服务器、调试适配器和离线分词可独立于 Oh My Pi 应用运行。AST 与 Hashline 预览在进程内生成统一补丁，使用常规的 `git apply --check` 和 `git apply` 检查和应用。

The MCP executable is `omp-toolkit-mcp`. The `omp-context-stats` CLI reads request usage from Codex rollout logs.

MCP 的启动命令为 `omp-toolkit-mcp`。`omp-context-stats` 用于统计 Codex 会话日志中的请求用量。

`omp-shell` and `obash` build from Oh My Pi v19.1.0. `omp-refactor learn` saves project lessons and produces portable Agent Skills drafts.

`omp-shell` 和 `obash` 基于 Oh My Pi v19.1.0 构建。`omp-refactor learn` 保存项目经验并生成可移植的技能草稿。

Sloppy and Magic preview commands use the released editing engines and emit Git patches for review. The connected coding agent supplies Magic's line operations.

Sloppy 和 Magic 预览命令采用已发布的编辑引擎，输出可审阅的 Git 补丁。Magic 的行操作由接入工具的编程智能体提供。

- [English installation guide](https://github.com/y26s4824k264/omp-toolkit/blob/main/README.md)
- [简体中文安装说明](https://github.com/y26s4824k264/omp-toolkit/blob/main/README.zh-CN.md)
