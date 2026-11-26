## Office and image reads / Office 与图片读取

- `office_read` reads DOCX blocks, PowerPoint slides, and Excel ranges with cell addresses, formulas, and cached values. Legacy XLS and PPT use LibreOffice for conversion.
- `image_read` returns metadata or a bounded preview. `media_read` batches known Office ranges and image crops for follow-up questions.
- Codex sessions with Code Mode can read known Office ranges and image crops in one `exec` call. The installer includes the routing instructions. [Optional Codex CLI profile](https://github.com/y26s4824k264/omp-toolkit/blob/v0.4.0/codex/omp-media.config.toml)

- `office_read` 按正文块、幻灯片或单元格范围读取 Office。Excel 保留单元格坐标、公式和缓存值。旧版 XLS 和 PPT 通过 LibreOffice 转换。
- `image_read` 返回图片元数据或限定范围的预览。`media_read` 批量读取已知范围，供后续提问复用。
- 提供 Code Mode 的 Codex 会话可用一次 `exec` 直读已知 Office 范围和图片裁剪区域。安装器附带调用指引。 [可选 Codex CLI 配置档](https://github.com/y26s4824k264/omp-toolkit/blob/v0.4.0/codex/omp-media.config.toml)

## Install / 安装

Prebuilt packages for macOS, Linux and Windows are available in Assets. The setup scripts verify package checksums and create local MCP configuration for WorkBuddy, Codex, Claude Code, Claude Desktop and other stdio clients.

Assets 提供 macOS、Linux 和 Windows 的 x64、ARM64 安装包。安装脚本校验包的哈希，并为 WorkBuddy、Codex、Claude Code、Claude Desktop 等支持 stdio 的客户端生成本地配置。

The one-command installers resolve the current stable version from the `VERSION` release asset. You can also pass an explicit version tag.

一键安装脚本通过发行页的 `VERSION` 文件获取当前稳定版，也可以显式指定版本。

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
