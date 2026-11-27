# Architecture & Extraction Philosophy

## Why this rewrite exists

[Oh My Pi (OMP)](https://github.com/can1357/oh-my-pi) is a full batteries-included coding agent with ~80k lines of Rust crates and a tightly coupled TypeScript agent runtime. Many of its best features depend intrinsically on owning the model loop (e.g., `Magic` edit's auxiliary `smol` model, `jfind`'s System One judge model, `TTSR` mid-stream token interception, `Advisor` sidecar turns, and `Goal` token-budget steering).

When extracting OMP capabilities for **external** coding agents (Antigravity, Claude Code, Codex, Cursor, VS Code, etc.), naively copying every subsystem produces bloat and conflicts:
- Wrapping `Magic` or `jfind` without auxiliary models turns time-saving helpers into multi-turn overhead.
- Exposing alternate file-editing tools (`hashline`/`sloppy`) fights the host agent's native diff UI, approval gates, and checkpoints.
- Bundling the 38k-line embedded bash 5.3 engine + 67 coreutils adds ~94 MB of dead weight on macOS/Linux just to run log filters.
- Spawning language servers from a short-lived CLI forces cold starts on every query.

`omp-toolkit` follows a strict extraction rule: **extract only the capabilities that fill genuine gaps in external agents, operate at 100% fidelity outside OMP's runtime, and never interfere with the host agent.**

---

## System Architecture

```text
                        ┌───────────────────────────────────────────────┐
                        │           Single Binary: omp-toolkit          │
                        │                 (~19 MB Rust)                 │
                        ├───────────────────────┬───────────────────────┤
                        │  `omp-toolkit mcp`    │  `omp-toolkit <cmd>`  │
                        │  (Stdio MCP Server)   │  (CLI + `setup`)      │
                        └───────────┬───────────┴───────────┬───────────┘
                                    │   100% in-process     │
                                    ▼                       ▼
┌─────────────────────┬───────────────────────┬───────────────────────┬──────────────────────┐
│  src/code_ast.rs    │     src/lsp/*         │      src/dap/*        │ src/minimize.rs      │
│  code_summary       │  Persistent LspPool   │  Persistent DapManager│  minimize_output     │
│   + block_line      │  Auto disk didChange  │  Stdio + TCP Socket   │ src/conflicts.rs     │
│  code_outline       │  Cross-platform rename│  14 OMP adapters      │  git_conflicts       │
│  ast_grep / ast_edit│  55 OMP LSP servers   │  Output ring buffer   │  (scan + resolve)    │
└─────────┬───────────┴───────────┬───────────┴───────────────────────┴──────────┬───────────┘
          │                       │                                              │
          ▼                       ▼                                              ▼
┌─────────────────────┐ ┌─────────────────────┐                       ┌────────────────────┐
│ crates/ast + xfs +  │ │    crates/diff      │                       │  crates/minimizer  │
│ vfs (from OMP)      │ │  In-memory unified  │                       │  Standalone crate  │
│ 17 tree-sitter langs│ │  diff (zero /tmp)   │                       │  (zero bash deps)  │
└─────────────────────┘ └─────────────────────┘                       └────────────────────┘
```

---

## Key Engineering Decisions

1. **Single Binary, Zero Subprocess Forking**:
   `src/lib.rs` exposes all engines directly. `omp-toolkit mcp` calls `code_ast`, `lsp`, `dap`, and `minimize` in-process, avoiding subprocess overhead and eliminating leaked `/tmp/omp-edit-*.patch` files.
2. **True Full-Repo AST Engine**:
   Uses `omp_ast::search::{discover, find_page, rewrite}` backed by `omp-xfs` parallel gitignore/worktree-aware directory walking rather than single-file wrappers. `omp-ast` builds with `default-features = false` so 17 compiled native tree-sitter grammars work immediately without dragging in `wasmtime` / `cranelift`.
3. **Standalone `omp-minimizer` Crate**:
   `crates/minimizer` extracts OMP `v19.1.0`'s `crates/shell/src/minimizer` (all 20+ Rust filters and 67 TOML pipeline definitions) and replaces its sole coupling (`crate::bash::syntax` in `plan.rs`) with a self-contained quote-aware lexical classifier. All 801 upstream unit tests pass in <0.1s without compiling bash 5.3.
4. **Host-Friendly Persistent LSP Pool**:
   Because external agents edit files through their own native tools, `LspClient::sync_file` and `reconcile_open_files` compare cached document text against disk before every query, automatically sending `textDocument/didChange` + `didSave`. `rename` and `rename_file` compute diffs purely in memory via `omp-diff` and write atomically on `apply: true` across macOS, Linux, and Windows.
5. **Unified Path Security (`src/path_guard.rs`)**:
   Every file/directory operation across AST, LSP, DAP, and `minimize_output` flows through one canonicalization guard that enforces root containment and blocks `.git` metadata access.
6. **Zero-Pollution Client Adapter (`src/setup.rs`)**:
   `omp-toolkit setup` manages client MCP configs and Antigravity's native plugin directory atomically, preserves unrelated servers, snapshots `PATH` for GUI apps, and strictly refuses to inject instructions into `CLAUDE.md`, `AGENTS.md`, or `GEMINI.md`.
