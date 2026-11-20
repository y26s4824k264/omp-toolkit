# OMP Toolkit

English · [简体中文](README.zh-CN.md)

Use standalone AST editing, Hashline, shell, LSP, and debugging tools in WorkBuddy, Codex, Claude, other coding agents, and the terminal. The local stdio MCP server reads source files, inspects logs, and previews patches. `omp-refactor` generates patches directly and writes diagnostics to stderr. Review each patch before applying it with Git.

The Rust engines and language-server defaults are extracted from [Oh My Pi v19.0.0](https://github.com/can1357/oh-my-pi/tree/v19.0.0), pinned at build time to `57771ae562b9345e0579bceb2d68d585e3e6927e`. The installed toolkit runs independently of the Oh My Pi application or any model account.

## Install

Download and run the script for your system from [GitHub Releases](https://github.com/y26s4824k264/omp-toolkit/releases/latest). The installer checks the archive's SHA-256, reuses a compatible Node.js and npm or downloads a verified copy, and installs the locked common language servers. Git is used to check and apply patches and for LSP rename previews.

You can also give your coding agent this instruction.

> Install OMP Toolkit using the installer for this system. Connect its local MCP tools to this agent and verify that `ast_find` is available. https://github.com/y26s4824k264/omp-toolkit

**macOS and Linux**

```sh
curl -fsSLo install.sh https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.2.0/install.sh && bash install.sh --version v0.2.0
```

The default install directory is `$HOME/.local/omp-toolkit`. Use `--prefix DIR` for a custom directory or `--without-lsp-servers` to skip the bundled list of language servers.

**Windows PowerShell 5.1 or later**

```powershell
Invoke-WebRequest -UseBasicParsing https://github.com/y26s4824k264/omp-toolkit/releases/download/v0.2.0/install.ps1 -OutFile install.ps1
& .\install.ps1 -Version v0.2.0
```

The default install directory is `%LOCALAPPDATA%\OMP Toolkit`. The options include `-Prefix DIR` and `-WithoutLspServers`. If machine policy restricts downloaded PowerShell scripts, follow your machine's or organization's process to review and allow the script.

The commands use versioned download URLs, so they work even when GitHub rate-limits anonymous API requests for release discovery. To choose another release, change the tag in both the URL and the installer argument.

The installer detects accessible local Codex and Claude Code CLIs and adds the MCP entry to each detected client. When the client's `omp` entry points to this installation, setup also adds a marked block of global tool instructions. Existing unrelated entries stay in place. Use `--client none` on macOS/Linux or `-Client none` on Windows for the tools and configuration files only. Use the printed command to add `bin` to your terminal's `PATH`.

The new default install directory is separate from the `v0.1.0` source installation. To switch clients to the new installation, replace the previous `omp` entry with the new one. If the installer cannot configure a client, it prints the command to run in your terminal.

### Platforms and features

| System | Release architectures | Requirements |
| --- | --- | --- |
| macOS | x64, arm64 | macOS 15+ |
| Linux | x64, arm64 | glibc distribution such as Ubuntu 22.04+ |
| Windows | x64, arm64 | PowerShell 5.1+ |

LSP symbol and file rename previews require macOS. Persistent `omp-debug` sessions on macOS and Linux require an adapter for the target language. Persistent DAP sessions are currently unavailable on Windows. V8 profiles can be read on all three systems, and `sample` reports come from macOS. Each agent has its own client requirements.

Prebuilt macOS packages use the system `open` command for opening files and URLs.

| Command | Purpose |
| --- | --- |
| `omp-toolkit-mcp` | 10 stdio MCP tools for AST, Hashline, logs, service readiness, and performance reports |
| `omp-refactor` | Structural search, patch previews, and source summaries |
| `omp-shell`, `obash` | Shell execution, log summaries, and Bash parsing |
| `omp-lsp` | Diagnostics, navigation, and semantic rename previews |
| `omp-debug` | Task-scoped breakpoint debugging through DAP |
| `codex-context-stats` | Local token usage stats from a specified Codex session log |

## Connect a coding agent

The installer writes `mcp-config.json` and `vscode-mcp.json` in the install directory with absolute paths for the launch command and arguments. When configuring a client manually, merge only the `omp` entry into its server configuration. A cloud session needs access to the execution environment running the tool.

For a client where you set tool preferences manually, copy the generated `AGENT-INSTRUCTIONS.md` into its global instructions.

| Agent | Setup |
| --- | --- |
| WorkBuddy | Open **Connectors → Custom connector → Add MCP** and merge `mcpServers.omp` from `mcp-config.json` into `mcp.json`. Save and confirm trust in WorkBuddy if prompted. See the download page for Linux availability. [Setup](https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Connector) · [Download](https://www.workbuddy.cn/work/) |
| Codex | Setup detects a local CLI. Run `codex mcp list` to check the configuration. [Official docs](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) |
| Claude Code | Setup detects a local CLI. Run `claude mcp get omp` to check the configuration. [Official docs](https://code.claude.com/docs/en/mcp#option-3-add-a-local-stdio-server) |
| Claude Desktop | In settings, open **Developer → Edit Config** and merge `mcpServers.omp` from `mcp-config.json`. Save, fully quit Claude Desktop, and restart. [Setup and paths](https://modelcontextprotocol.io/docs/develop/connect-local-servers) |
| Cursor and other local MCP clients | For Cursor, merge `mcpServers.omp` into `~/.cursor/mcp.json`. For other clients that support local stdio, use the `command` and `args` from the generated configuration. [Cursor docs](https://prod.cursor.com/help/customization/mcp) |
| VS Code and Copilot | For editor-managed VS Code configuration, use `servers.omp` from `vscode-mcp.json`. For the current Copilot harness, add the server through **Chat: Open Customizations → MCP Servers**, or merge `mcpServers.omp` from `mcp-config.json` and add `"tools": ["*"]` to the server entry. [Configuration docs](https://code.visualstudio.com/docs/agents/reference/mcp-configuration) |

In a new session, ask the agent to call `ast_find` on a test project or `hashline_read` on a test file. A preview returns the full patch. Inspect it and run `git apply --check` before applying it.

## Structural edits

List the target files from the project root. Review the matches and the patch, then check and apply it. Generate a new patch if the target files have changed since the preview.

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

AST matching works on syntax. Use the language server to check which symbol a reference belongs to. In Go, for example, run `gopls rename -d file.go:line:column NewName` for a preview, then `-w` to write. The extracted engine includes OMP's built-in grammars for Go, TypeScript, Rust, and other languages.

`ast-find` reports matching text, location, captures, and pagination information without changing files. Use `--offset` for subsequent pages. Results follow the order of the input files and the position of each match within its file.

## Hashline edits

Run `omp-refactor read src/a.ts --start 10 --end 18` and copy its `[path#tag]` header into your edit. For a large file, use `omp-refactor read-summary src/a.ts` to find declarations and their line numbers, then read the range to get a current tag. The tag and line number below are examples.

```text
*** Begin Patch
[src/a.ts#A1B2]
PUT 12.=12:
+const value = calculate();
*** End Patch
```

Run `omp-refactor hashline --patch edits.txt src/a.ts` to generate a Git patch. Hashline handles existing text files and checks the current tag before preparing edits. Read new anchors after any source change. Use your project's regular editing tools to create, move, or remove files.

## Build from source

On macOS or Linux, the source installer needs Git, Rustup, system build tools, and Node.js 24+. The common language servers also require npm in this build mode.

```sh
git clone https://github.com/y26s4824k264/omp-toolkit.git
cd omp-toolkit
./scripts/install.sh --with-lsp-servers
```

The Rust toolchain is pinned to upstream `nightly-2026-11-08`. Run the following in the repository root to build the Rust toolkit.

```sh
cargo build --release --locked -p omp-refactor
```

The binary is in `target/release/omp-refactor`. It uses Git at runtime.

## Shell commands and logs

`omp-shell` calls OMP 19.0.0's native process shell, with Bash, Zsh, POSIX sh, and native exit codes on supported platforms. `obash` is the upstream command for executing, parsing, or tracing Bash. [Rust process shell](https://github.com/can1357/oh-my-pi/blob/v19.0.0/crates/shell/src/process/mod.rs) · [obash source](https://github.com/can1357/oh-my-pi/blob/v19.0.0/crates/shell/src/bin/obash/main.rs)

```sh
omp-shell bash -c 'printf "%s\n" alpha beta | grep alpha'
omp-shell zsh -f -c 'print -r -- "$ZSH_VERSION"'
obash --json=native -c 'printf "%s\n" alpha | grep alpha'
```

`run-log` runs a long command once, saves combined stdout and stderr, and returns a short preview with the original exit code. Without `--log`, it creates a private temporary log and prints its path before running. A new file given with `--log` also uses private permissions. The command and arguments pass through as supplied. Use `bash -c` or `zsh -c` explicitly for pipes and redirection.

```sh
omp-shell run-log -- go test ./...
omp-shell run-log --log ./test-output.log -- npm test
```

`minimize` summarizes an existing log and reports the exit code, the sizes before and after filtering, and the original path.

```sh
log_file=$(mktemp)
test_status=0
go test ./... > "$log_file" 2>&1 || test_status=$?
omp-shell minimize "$log_file" "$test_status" 'go test ./...'
```

To build the shell manually, copy `shell/omp_shell_cli.rs` as `main.rs` and `src/log_preview.rs` as `log_preview.rs` into the upstream v19.0.0 directory `crates/shell/examples/omp_shell_cli/`. Then run these commands from the upstream repository root.

```sh
cargo build --locked --profile ci -p omp-shell --example omp_shell_cli
cargo build --locked --profile ci -p omp-shell --bin obash
```

Install `target/ci/examples/omp_shell_cli` as `omp-shell`. The upstream `obash` binary is `target/ci/obash`.

Use `omp-shell process-tree PID --depth 2 --json` to inspect a long-running task and its child processes. The report includes process IDs and status and omits command-line arguments.

## Other standalone capabilities

- `omp-refactor ranked-find 'request timeout retry' --limit 8` ranks files by keyword coverage and rarity and returns matching lines with their file paths. Use `rg` for exact lookups.
- `omp-refactor token-count FILE` estimates counts offline with `o200k_base`, `cl100k_base`, and four Claude encodings. Measure actual Codex input usage from the session log.
- `omp read FILE:5-12` reads local source lines. `omp grep PATTERN PATH --limit 20` searches local text.
- The extracted Rust engines include `omp-diff` and `omp-xfs` for diffs and file traversal.

## MCP tools

The stdio entry point is `omp-toolkit-mcp`. In Codex, look for the `mcp__omp__` tools. In other clients, find the `omp` server. Use an absolute project directory for `root` and paths within that directory for `file`, `files`, and `log`.

| Tool | Purpose | JSON fields |
| --- | --- | --- |
| `ast_find` | Find syntax matches, locations, and captures | `root`, `files`, `language`, `pattern`. Optional `limit` and `offset` for pagination. |
| `ast_preview` | Preview a structural replacement as a full Git patch | `root`, `files`, `language`, `pattern`, `replacement` |
| `read_summary` | Read a source outline and original line numbers | `root`, `file` |
| `hashline_read` | Read original lines and current tags, up to 120 lines per call | `root`, `file`, `start`, `end`. The 1-based endpoints are inclusive. |
| `hashline_preview` | Preview `PUT` and `CUT` edits with current anchors | `root`, `files`, `patch`. Match the file list to the patch headers. |
| `shell_parse` | Parse a Bash syntax tree without running the script | Bash source in `script` |
| `log_minimize` | Summarize an existing command log, preserving its path and exit code | `root`, `file`, `command`, `exit_code` |
| `service_ready` | Wait for a local port or startup log and detect early process exit | `root`. Provide `port`, the log fields `log`/`log_pattern`/`log_offset`, or both. `pid` and `timeout_ms` are optional. |
| `profile_summary` | Summarize a V8 CPU profile or macOS `sample` report | `root`, `file` |
| `token_count` | Estimate file tokens with local tokenizers | `root`, `file`. Use `codex-context-stats` for actual Codex request usage. |

To inspect repeated calls first, call `ast_find`.

```json
{"root":"/absolute/project","files":["src/a.ts","src/b.ts"],"language":"typescript","pattern":"oldCall($VALUE)","limit":20}
```

For lengthy results, read the needed fields from the returned `Full JSON` path. `ast_preview` and `hashline_preview` return a `Full patch` path. Inspect the complete patch, run `git apply --check -- "$patch"` from the project root, then run `git apply -- "$patch"`. Preview again if the source changes. Failed logs and logs larger than 4 MiB use a head and tail excerpt. Other logs go through OMP's command-aware filter.

The equivalent terminal tools are `omp-refactor ast-find`, `ast`, `read-summary`, `read`, `hashline`, and `token-count`. Use `obash --json=native -c '…'` to parse Bash and `omp-shell minimize` to summarize logs. To connect a source build to Codex, use absolute paths.

```sh
cargo build --release --locked -p omp-refactor --bin omp-toolkit-mcp
codex mcp add omp -- /absolute/path/to/omp-toolkit-mcp \
  --bin-dir /absolute/directory/with/native-tools \
  --node /absolute/path/to/node --profile-helper /absolute/path/to/profile/summary.mjs
```

For a series of shell steps in Codex, run `exec omp-shell zsh -f -i` in a task-specific PTY. Continue in the same session to retain the working directory, variables, and functions. Use `$?` to read a command's status, and `exit` when finished.

### Local service readiness

Before starting the process, record the existing log length or create a fresh log and use offset `0`. After launch, call `service_ready` with the actual loopback port or the log condition. It matches complete lines written after the recorded offset. If you provide both a port and a log condition, both must pass. Supply the task PID to detect early exit. A listening port confirms local connectivity. Use an application health endpoint when you need to verify application behavior.

```json
{"root":"/absolute/project","port":5173,"log":"work/dev.log","log_pattern":"ready in","log_offset":0,"timeout_ms":10000}
```

### Performance reports

`profile_summary` reads an existing `.cpuprofile` or macOS `.sample.txt` file inside the project and reports hot paths, call trees, and threads. The input limit is 32 MiB. For a manual install, copy the entire `profile/` directory and use Node.js 24 for TypeScript type stripping.

### Language diagnostics and navigation

`omp-lsp` starts language servers directly over LSP stdio. `catalog` lists the 55 extracted definitions, and `status` reports the project configuration. `check` can reuse a server across explicitly listed files. Exit code `0` means a report arrived with no errors, `1` means reported errors, and `2` means the check failed or returned an incomplete report.

```sh
omp-lsp --json catalog | jq -r '.servers[].name'
omp-lsp --root /absolute/project status
omp-lsp --root /absolute/project check src/main.ts src/helper.ts
omp-lsp --root /absolute/project definition src/main.ts --line 12 --symbol load
```

Keep the `lsp/` runtime modules together and use Node.js 24+. For a manual install of the common language servers, run `npm ci --ignore-scripts` in `lsp/servers`. The engine checks project-local dependencies, installed language servers, and `PATH`. Bash lint diagnostics use ShellCheck when available. Go and Rust still use their project-selected toolchains. Increase `--timeout` for a cold start. Follow your project's requirements for builds and tests.

### Semantic rename previews

Project language server settings use `.omp/lsp.json`, `.lsp.json`, and `lsp.json` in precedence order, with later files overriding earlier ones. Move settings needed from an Oh My Pi user configuration or YAML to a project JSON file.

`rename-preview` asks the language server to locate a definition and references. `rename-file-preview` previews a file move and the corresponding import edits. Giving only a filename keeps the current directory. A multi-component path is relative to the project root. On macOS, both commands produce a full Git patch in a private temporary file by default. `--patch` selects a new output file. `--json` returns the patch path, preview, and Git `apply_root`. The supported edits depend on the language server.

```sh
patch_dir=$(mktemp -d)
omp-lsp --root /absolute/project rename-preview src/config.ts \
  --line 12 --symbol oldName --new-name newName --patch "$patch_dir/rename.patch"
git -C /absolute/project apply --check -- "$patch_dir/rename.patch"

omp-lsp --root /absolute/project rename-file-preview src/config.ts --new-name settings.ts
```

Review the full patch and run `git apply --check` from `apply_root`. `--root` may point to a nested project. Patch paths still start at its Git root. Preview again after source changes.

### Persistent breakpoint debugging

`omp-debug` talks directly to debug adapters over DAP and retains breakpoints, stack frames, and variables during a task. Persistent sessions run on macOS and Linux with Node.js 24 and a debug adapter for the language. On macOS, C and C++ can use `lldb-dap` from Xcode or Command Line Tools.

For a manual install, keep `debug/omp-debug.mjs`, `debug/bridge.mjs`, and `debug/protocol.mjs` together. Link `omp-debug.mjs` as `omp-debug` on `PATH`.

Run the server in the foreground in a task-specific PTY or Codex Process Jobs. Use an unused directory for `--session` and pass the same absolute path to every subsequent command.

```sh
debug_dir=$(mktemp -d)
session="$debug_dir/session"
omp-debug --session "$session" serve --root /absolute/project
```

Wait for `OMP_DEBUG_READY`, then send commands from a separate terminal. Build the target with debugging symbols. Read frame IDs and variable references from the previous command's JSON. Use `pause` to interrupt a running program and `stop` to end the session. Idle sessions stop after 15 minutes unless you set `serve --idle` in seconds.

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

### Codex context usage

`codex-context-stats` reads the specified Codex JSONL file with Node.js. It reports input and cached tokens for the first model request, the share of the context window used, and input growth across later requests in the same turn. Use `--turns` to list turns and `--requests` for individual requests. `--turn` takes a 1-based turn number or a turn ID.

```sh
codex-context-stats --turn 1 --requests /absolute/path/to/rollout.jsonl
```

## License

The project adapters use the [MIT License](LICENSE). The extracted upstream code and third-party fixtures retain their original licenses. The [third-party notices](THIRD_PARTY_NOTICES.md) list their sources. The toolkit is independently maintained.
