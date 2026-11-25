# OMP Toolkit

English · [简体中文](README.zh-CN.md)

Use Oh My Pi's code inspection, patch previews, shell tools, LSP diagnostics, and debugging from WorkBuddy, Codex, Claude, or your terminal. The toolkit runs locally and needs no Oh My Pi installation or model account of its own.

## Install v0.3.0

The installer verifies the binary archive against its SHA-256 checksum. It reuses compatible Node.js and npm on `PATH` or downloads verified upstream copies. Common language servers install by default. Install Git for patch validation, patch application, and LSP rename previews. [Release and checksums](https://github.com/y26s4824k264/omp-toolkit/releases/tag/v0.3.0)

**macOS and Linux**

```sh
curl -fsSLo install.sh https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.3.0/install.sh && bash install.sh --version v0.3.0
```

**Windows PowerShell 5.1 or later**

```powershell
Invoke-WebRequest -UseBasicParsing https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.3.0/install.ps1 -OutFile install.ps1; if ($?) { & .\install.ps1 -Version v0.3.0 }
```

The default directories are `$HOME/.local/omp-toolkit` and `%LOCALAPPDATA%\OMP Toolkit`. Use `--prefix DIR` or `-Prefix DIR` for a custom location. Use `--without-lsp-servers` or `-WithoutLspServers` to skip the common language servers. Follow the installer output to add `bin` to `PATH`, then run `omp-refactor --version` and `omp-toolkit-mcp --version`.

For an install that leaves client settings untouched, run the downloaded script with `none`.

```sh
bash install.sh --version v0.3.0 --client none
```

```powershell
& .\install.ps1 -Version v0.3.0 -Client none
```

| Platform | Architectures | Requirements |
| --- | --- | --- |
| macOS | x64, arm64 | macOS 15+ |
| Linux | x64, arm64 | glibc distribution, such as Ubuntu 22.04+ |
| Windows | x64, arm64 | PowerShell 5.1+ |

## Connect an agent

By default, the installer detects Codex and Claude Code CLIs on `PATH` and registers the local stdio server as `omp`. Use `--client codex` or `--client claude` to select one on macOS/Linux, or `-Client codex` or `-Client claude` on Windows. An existing `omp` entry that points elsewhere is preserved. When a client points to this installation, the installer adds a marked block of usage guidance to that client's global instructions.

For manual setup, the install directory contains `mcp-config.json`, `vscode-mcp.json`, and `AGENT-INSTRUCTIONS.md`. Merge the generated `omp` entry into the client's existing server list. The command and arguments already use absolute paths to this install.

| Client | Setup and verification |
| --- | --- |
| Codex | Run `codex mcp list` to check registration. [Docs](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) |
| Claude Code | Run `claude mcp get omp` to check registration. [Docs](https://code.claude.com/docs/en/mcp#option-3-add-a-local-stdio-server) |
| WorkBuddy | In **Connectors → Custom connector → Add MCP**, merge `mcpServers.omp` into `mcp.json`. Confirm trust in WorkBuddy if prompted. [Docs](https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Connector) |
| Claude Desktop | In **Developer → Edit Config**, merge `mcpServers.omp`, then restart the app. [Docs and config paths](https://modelcontextprotocol.io/docs/develop/connect-local-servers) |
| Other stdio clients | Use the generated `command` and `args`. Cursor and current Copilot use `mcpServers.omp`. For Copilot add `"tools": ["*"]` to the entry. VS Code editor-managed config uses `servers.omp`. [Cursor](https://prod.cursor.com/help/customization/mcp) · [VS Code and Copilot](https://code.visualstudio.com/docs/agents/reference/mcp-configuration) |

Open a new session and ask the agent to run `ast_find` on a test project. In Codex, the tools use the `mcp__omp__` prefix. A remote session needs access to the machine or environment where the toolkit is installed.

## Inspect and preview changes

Run the examples from a Git project root. The edit examples assume `src/a.ts` contains `oldCall(7)` on line 1. Use `--root` to select another project. For MCP calls, `root` must be absolute and file fields are project-relative.

**AST** matches syntax and can preview the same rewrite across files.

```sh
omp-refactor ast-find --lang typescript --pattern 'oldCall($VALUE)' src/a.ts
omp-refactor ast --lang typescript --pattern 'oldCall($VALUE)' --rewrite 'newCall($VALUE)' src/a.ts > ast.patch
git apply --check -- ast.patch
```

**Hashline** checks a patch against the current file tag. Use `omp-refactor read-summary` to locate code and `omp-refactor read` to get fresh line anchors. **Sloppy** uses text anchors or syntax-aware block operations. Supply every file named in the edit. [Sloppy syntax](https://github.com/can1357/oh-my-pi/blob/v19.1.0/crates/edit/prompts/sloppy.md)

```sh
printf '%s\n' '*** Edit File: src/a.ts' '*** Find' 'oldCall(7)' '*** Replace' 'newCall(7)' > change.edit
omp-refactor sloppy --patch change.edit src/a.ts > sloppy.patch
git apply --check -- sloppy.patch
```

**Magic** turns a description into a plan for your coding agent. Prepare with a `# relative/path` heading per file. Give the agent `systemPrompt` and each file's `request` from the plan. These may contain project source. Save the returned line operations in `replies.json`, then run `preview` to validate them against the original snapshot. Use `prepare --full` if an excerpt omits necessary context. [Magic sources](magic/UPSTREAM.md)

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

The previews leave source files untouched. Inspect the complete diff and apply it with `git apply -- PATH.patch` when satisfied. Prepare a new preview if the source changes.

## Save project lessons and skill drafts

Use `--root` to choose which project owns the records. `save` deduplicates lessons, `list` reads them, and `skill preview` prints a draft. `create` saves a new `SKILL.md` under `.omp-toolkit/learning/skills/`. `update` intentionally replaces a draft you already created. Review drafts before installing them in a client.

```sh
omp-refactor --root . learn save 'Check the complete Git patch before applying it.'
omp-refactor --root . learn list
printf '%s\n' 'Review the diff, then run git apply --check.' > patch-review.md
omp-refactor --root . learn skill preview patch-review --description 'Review a Git patch' --body-file patch-review.md
omp-refactor --root . learn skill create patch-review --description 'Review a Git patch' --body-file patch-review.md
omp-refactor --root . learn skill update patch-review --description 'Review a Git patch' --body-file patch-review.md
```

## Track a long-running goal

The current source build includes `omp-refactor goal`. It stores a goal ID, its objective and acceptance criteria, and a separate current milestone in `.omp-toolkit/goals/`. Use the same project root and ID to resume it. Hosts like Codex can keep their native goal active while the project record tracks milestone and evidence details. The structure follows Oh My Pi's separate goal and todo state. [Oh My Pi goal continuation](https://github.com/can1357/oh-my-pi/blob/v19.1.1/packages/coding-agent/src/prompts/goals/goal-continuation.md) · [Todo state](https://github.com/can1357/oh-my-pi/blob/v19.1.1/packages/coding-agent/src/prompts/goals/goal-todo-context.md)

```sh
omp-refactor --root . goal init release --objective 'Publish and verify the update' --criterion 'Platform tests pass' --criterion 'Public installation works'
omp-refactor --root . goal focus release 'Run the platform tests'
omp-refactor --root . goal record release 1 'CI run 123 passed the required platform jobs'
omp-refactor --root . goal show release
```

`goal list` shows saved IDs. `goal clear-evidence release 1` withdraws stale evidence. `ready_for_review` indicates every criterion has a recorded reference. Confirm that those references still prove the objective before closing the host goal. The v0.3.0 bundles predate this command.

## Other commands

| Workflow | Entry point |
| --- | --- |
| Locate files by search terms | `omp-refactor ranked-find "keywords" --limit 10` |
| Run commands and retain complete logs | `omp-shell run-log -- COMMAND ARGS...`. Use `omp-shell minimize` for existing logs and `obash --json=native -c '…'` to parse Bash syntax. |
| Check language servers | `omp-lsp catalog`, `omp-lsp status`, `omp-lsp check FILE...`, or `omp-lsp definition`. Diagnostics depend on the project's language server. Symbol and file rename previews require macOS. |
| Debug with a persistent session | `omp-debug --session ABSOLUTE_DIR serve --root PROJECT`, then send `launch`, `break`, `continue`, or `stop` from another terminal. macOS and Linux require a suitable DAP adapter. Windows support currently excludes persistent DAP sessions. |
| Measure recorded context usage | `omp-context-stats --turn 1 --requests ROLLOUT.jsonl` reads Codex rollout JSONL. |
| Inspect local runtime behavior through MCP | `service_ready` checks a loopback port or new log lines. `profile_summary` reads V8 or macOS `sample` reports. `token_count` estimates file tokens offline. |

Use `--help` on the command or subcommand for parameters. The MCP client shows tool argument schemas.

## Upgrade from earlier releases

Use `omp-context-stats` in scripts that previously called `codex-context-stats`. Set hand-written MCP configurations to `omp-toolkit-mcp` or `omp-toolkit-mcp.exe` on Windows. The old `omp-codex-mcp` alias is absent from new installs. The generated configuration includes the correct executable path and arguments.

## Source and licenses

To build from source on macOS or Linux, install Git, Rustup, system build tools, Node.js 24+, and npm if you want the common language servers. Then run `./scripts/install.sh --with-lsp-servers` from the checkout. Use the tagged release to match a specific version. [Source installer](scripts/install.sh) · [Release workflow](.github/workflows/release.yml)

This is an independently maintained MIT project. Base editing and LSP defaults come from Oh My Pi v19.0.0. Shell and `obash` build from Oh My Pi v19.1.0, which also supplies selected filesystem and editing fixes. Binary archives include dependency notices. The release page provides their checksums. [License](LICENSE) · [Third-party notices and exact upstream revisions](THIRD_PARTY_NOTICES.md)
