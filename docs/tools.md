# MCP Tools & CLI Reference

All 8 MCP tools execute **in-process** inside the single `omp-toolkit` binary (no subprocess forking, no `/tmp` patch file leaks). The CLI exposes matching commands backed by the exact same Rust modules.

---

## 1. `code_summary` — Tree-sitter AST-Folded Reading & Block Expansion

Two complementary low-token reading modes in one tool:
- **Folded summary mode (default)**: folds function/method bodies and long block comments into concise elision markers while preserving exact 1-based line numbers, with progressive BFS unfolding (`unfold_until_lines`).
- **Syntactic block mode (`block_line`)**: powered by `omp_ast::block` — given any 1-based line number, expands that complete tree-sitter block (including Rust outer `#[...]` attributes / `///` doc comments and Python/TS/Java decorators). If `block_line` lands inside a block or on a closing delimiter, it automatically resolves the tightest enclosing multi-line block.

**Parameters**
| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `root` | `string` (abs path) | required | Absolute project root directory. |
| `file` | `string` (rel path) | required | Project-relative source file. |
| `lang` | `string?` | auto | Explicit language alias (`rust`, `typescript`, `python`, `go`, etc.). |
| `block_line` | `u32?` | none | 1-based line to expand into its complete tree-sitter syntactic block instead of folding the whole file. |
| `unfold_until_lines` | `u32?` | `120` | Target visible line budget for BFS unfolding (`0` = outermost fold only). |
| `unfold_limit_lines` | `u32?` | `240` | Hard ceiling for unfolded visible lines. |
| `min_body_lines` | `u32?` | `4` | Minimum node lines before folding a body. |
| `min_comment_lines` | `u32?` | `6` | Minimum lines before folding a block comment. |
| `compact` | `bool?` | `true` | `true` emits compact numbered lines; `false` emits structured JSON. |

**CLI**:
- `omp-toolkit summary src/code_ast.rs --unfold-until 120`
- `omp-toolkit summary src/code_ast.rs --block-line 135`

---

## 2. `code_outline` — Instant Tree-sitter Symbol Map

Lists top-level symbols (functions, structs, classes, interfaces, enums, methods, fields) with exact 1-based `start_line`/`end_line` and signatures — zero LSP startup cost.

**Parameters**
| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `root` | `string` (abs path) | required | Absolute project root directory. |
| `file` | `string` (rel path) | required | Project-relative source file. |
| `lang` | `string?` | auto | Optional language override. |
| `types` | `string[]?` | all | Filter by symbol kinds (`function`, `method`, `struct`, `class`, `interface`, `enum`, `constant`, etc.). |
| `include_imports` | `bool?` | `false` | Whether to include import/use declarations. |

**CLI**: `omp-toolkit outline src/mcp.rs --type function`

---

## 3. `ast_grep` — Full-Repository Parallel AST Search

Uses `omp-ast` + `omp-xfs` parallel directory discovery (honoring `.gitignore`, linked Git worktree `info/exclude`, and skipping `node_modules`/`.git`) to search syntax patterns across an entire repository.

**Parameters**
| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `root` | `string` (abs path) | required | Absolute project root directory. |
| `path` | `string?` | `root` | Optional subdirectory or single file scope. |
| `patterns` | `string[]` | required | One or more ast-grep patterns (`$VAR` single capture, `$$$ARGS` variadic capture). |
| `lang` | `string?` | auto | Omit to auto-detect per file extension across the repo. |
| `selector` | `string?` | none | Tree-sitter node kind for fragment patterns (e.g. `pair`, `field_declaration`). |
| `include` | `string?` | none | Glob include filter (e.g. `src/**/*.rs`, `*.ts`). |
| `exclude` | `string[]?` | none | Glob exclude filters (e.g. `["*.test.ts", "fixtures"]`). |
| `strictness` | `string?` | `smart` | One of `smart`, `cst`, `ast`, `relaxed`, `signature`. |
| `hidden` | `bool?` | `false` | Include dot-prefixed entries. |
| `offset` / `limit` | `usize?` | `0` / `30` | Pagination across total matches (`limit`: 1–100). |

**CLI**: `omp-toolkit ast-grep --pattern 'oldCall($$$ARGS)' --include 'src/**/*.ts'`

---

## 4. `ast_edit` — Full-Repository Parallel AST Rewrite

Applies ordered structural `{ pattern, rewrite }` rules across matching files, validates every rewritten file with tree-sitter before touching disk, and preserves original CRLF/LF line endings.

**Parameters**
- Same discovery/matching fields as `ast_grep` (`root`, `path`, `lang`, `selector`, `include`, `exclude`, `strictness`), plus:
- `rules`: `Array<{ pattern: string, rewrite: string }>`
- `skip_overlaps` (`bool?`, default `false`)
- `max_replacements` (`usize?`, default `500`), `max_files` (`usize?`, default `100`)
- **`apply`** (`bool?`, default **`false`**):
  - `false`: dry-run in memory and return standard unified `diff` + per-file replacement counts.
  - `true`: validate syntax across every changed file first, then atomically write all files.

**CLI**: `omp-toolkit ast-edit --pattern 'old($X)' --rewrite 'new($X)' [--apply]`

---

## 5. `lsp` — Persistent Workspace LSP Pool

Reuses warm language servers per `(server, workspace_root)` across tool calls and automatically reconciles external disk edits (`didChange` + `didSave`) before every query so it stays in sync with your host agent's native file edits.

Built-in catalog: **55 language servers** from Oh My Pi's `defaults.json`, overridable per project via `.omp/lsp.json`, `.lsp.json`, or `lsp.json`.

**Actions**
| `action` | Key Parameters | Behavior |
| --- | --- | --- |
| `status` | `root` | Lists warm pool connections and locally installed servers matching project root markers. |
| `diagnostics` | `root`, `file` or `files` | Pulls `textDocument/diagnostic` or waits for debounced `publishDiagnostics` from primary server + linters. |
| `definition`, `references`, `hover`, `type_definition`, `implementation` | `root`, `file`, (`symbol` and/or `line` + `column`) | Navigates or inspects types. Passing `symbol` (or `name#2` + `line`) resolves UTF-16 columns automatically. |
| `symbols` | `root`, `file` | Hierarchical document symbols from the language server. |
| `workspace_symbols` | `root`, `query` | Searches symbols across the whole workspace. |
| `code_actions` | `root`, `file`, (`symbol` / `line`) | Lists quick fixes and refactors available at the target range. |
| `rename` | `root`, `file`, `symbol`/`line`, `new_name`, `apply?` | Cross-file semantic rename. Default `apply: false` previews unified diff; `apply: true` writes atomically on macOS, Linux, and Windows. |
| `rename_file` | `root`, `file`, `new_name`, `apply?` | Queries `workspace/willRenameFiles` to update imports/re-exports, previews diff by default, or moves file + applies edits atomically with `apply: true`. |
| `shutdown` | `root` | Gracefully stops all warm language servers in the pool. |

**CLI**: `omp-toolkit lsp status`, `omp-toolkit lsp diagnostics src/main.rs`

---

## 6. `debug` — Persistent Multi-Session DAP Debugger

Maintains live debug sessions inside the MCP server process across turns, supporting both **stdio** adapters (`lldb-dap`, `gdb -i dap`, `debugpy`, etc.) and **TCP socket** adapters (`dlv dap`, `codelldb --port 0`).

Built-in catalog: **14 DAP adapters** from Oh My Pi's `defaults.json`, overridable via `.omp/dap.json`, `.dap.json`, or `dap.json`. On macOS, `lldb-dap` is auto-discovered via `xcrun --find lldb-dap`.

**Actions**
- Discovery & lifecycle: `adapters`, `sessions`, `launch` (`root`, `program`, `args?`, `adapter?`, `stop_on_entry?`), `attach` (`root`, `pid?`, `host?`, `port?`, `adapter?`), `terminate`.
- Breakpoints: `set_breakpoint` (`file`, `line`, `condition?`, `hit_condition?`), `remove_breakpoint` (`file`, `line?`).
- Execution control: `continue`, `pause`, `step_over`, `step_in`, `step_out`.
- State inspection: `threads`, `stack_trace` (`levels?`), `scopes` (`frame_id`), `variables` (`variable_ref`), **`evaluate`** (`expression`, `frame_id?`, `context?`), **`output`** (`clear?`).

**CLI**: `omp-toolkit debug-adapters` (interactive multi-step sessions run over `omp-toolkit mcp`)

---

## 7. `minimize_output` — Command-Aware CLI Output Minimizer

Standalone extraction of Oh My Pi's output minimizer (20+ Rust filters + 67 declarative TOML pipelines adapted from OMP/RTK) covering `cargo`, `git`, `pytest`, `jest`, `vitest`, `tsc`, `eslint`, `ruff`, `go`, `docker`, `kubectl`, `gradle`, `maven`, `dotnet`, `rspec`, `terraform`, etc.

Unlike the previous wrapper, it **works on non-zero exit codes** so failing test/compiler logs are condensed directly to the failing tests, stack traces, and summary counts.

**Parameters**
| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `command` | `string` | required | Original command line used to select the filter (e.g. `cargo test`, `git diff`). |
| `exit_code` | `i32?` | `0` | Exit code of the command. |
| `output` | `string?` | — | Raw output text (provide either `output` or `root` + `file`). |
| `root` / `file` | `string?` | — | Read raw output from a saved log file inside `root` (up to 4 MiB). |
| `min_chars` | `usize?` | `0` | Skip filtering if input is shorter than this threshold. |

**CLI**: `cargo test 2>&1 | omp-toolkit minimize --cmd "cargo test" --exit-code 101`

---

## 8. `git_conflicts` — Full-Repository Git Merge Conflict Scanner & Structured Resolver

Inspired by Oh My Pi's `conflict://` workflow. Uses `omp-xfs` parallel gitignore-aware walking to detect strict column-0 Git conflict blocks (`<<<<<<<` / optional diff3 `|||||||` / `=======` / `>>>>>>>`), and resolves individual or batched conflicts cleanly without risking leftover conflict markers or broken CRLF line endings.

**Actions**
- `scan`: Scans a single `file`, subdirectory, or the entire repository (`root`), returning 1-based conflict indices, line ranges, branch labels, and `ours` / `base` / `theirs` sections.
- `resolve`: Resolves one conflict (`index` + `choice`), all conflicts in `file` (`all: true` + `choice`), or multiple conflicts with distinct choices in one atomic batch (`resolutions: [{ index, choice, custom_text? }]`).
  - Supported `choice` values: `ours`, `theirs`, `base` (diff3 only), `both`, `both_reverse`, `delete`, `custom`.
  - When `choice` is `custom`, standalone lines `@ours`, `@theirs`, `@base`, and `@both` inside `custom_text` expand to the corresponding recorded sides.
  - Defaults to **`apply: false`** (returns unified diff preview + `remaining_conflicts`); pass **`apply: true`** to write atomically.

**CLI**:
- `omp-toolkit conflicts scan`
- `omp-toolkit conflicts resolve src/app.rs --index 1 --choice ours [--apply]`
- `omp-toolkit conflicts resolve src/app.rs --all --choice theirs --apply`
