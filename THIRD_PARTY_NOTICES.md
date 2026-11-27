# Third-party notices

The toolkit's MCP server, CLI, LSP/DAP engines, and glue code are licensed under the root [MIT License](LICENSE). Third-party components extracted from upstream projects retain their respective licenses:

| Included material | Upstream source | License |
| --- | --- | --- |
| `crates/ast`, `crates/diff`, `crates/vfs`, `crates/xfs` | [Oh My Pi](https://github.com/can1357/oh-my-pi) (`v19.0.0` base + selected `v19.1.0`/`v19.2.0`/`v19.2.1` fixes, see each crate's `UPSTREAM.md`) | MIT ([LICENSE](LICENSE), [crates/ast/LICENSE](crates/ast/LICENSE), [crates/xfs/LICENSE](crates/xfs/LICENSE)) |
| `crates/minimizer` | Extracted standalone from [Oh My Pi v19.1.0 (`a7135b8`)](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e/crates/shell/src/minimizer), with embedded bash AST dependency replaced by a standalone lexical classifier (`plan.rs`). Includes RTK-adapted filters. | MIT ([crates/minimizer/LICENSE](crates/minimizer/LICENSE), [crates/minimizer/NOTICE](crates/minimizer/NOTICE)) |
| `assets/lsp-defaults.json`, `assets/dap-defaults.json` | [Oh My Pi v19.1.0 (`a7135b8`)](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e/packages/coding-agent/src) | MIT |
| `crates/vendor/grep-searcher` | `grep-searcher` 0.1.17 via Oh My Pi vendor tree | MIT / Unlicense ([LICENSE-MIT](crates/vendor/grep-searcher/LICENSE-MIT), [UNLICENSE](crates/vendor/grep-searcher/UNLICENSE)) |
| `crates/vendor/tree-sitter-go` | `tree-sitter-go` 0.25.0 via Oh My Pi vendor tree | MIT ([LICENSE](crates/vendor/tree-sitter-go/LICENSE)) |
| `crates/ast/src/language/testdata/ini.wasm` | `tree-sitter-ini` 1.4.0 test fixture | Apache-2.0 ([licenses/tree-sitter-ini-1.4.0-APACHE-2.0.txt](licenses/tree-sitter-ini-1.4.0-APACHE-2.0.txt)) |
