# Third-party notices

The toolkit's adapters are provided under the root [MIT License](LICENSE). The third-party files listed below retain their respective licenses.

| Included material | Source | License files |
| --- | --- | --- |
| Rust editing and filesystem engines in `crates/ast`, `crates/diff`, `crates/edit`, `crates/vfs`, and `crates/xfs` | [Oh My Pi v19.0.0, commit `57771ae562b9345e0579bceb2d68d585e3e6927e`](https://github.com/can1357/oh-my-pi/tree/57771ae562b9345e0579bceb2d68d585e3e6927e). The extracted manifests and sources were adapted for the standalone toolkit. | MIT. [Root license](LICENSE), [AST license](crates/ast/LICENSE), and [XFS license](crates/xfs/LICENSE). The INI test fixture has its own license. |
| `profile/cpuprofile.ts`, `profile/profile-tree.ts`, and `profile/sample-profile.ts` | The same Oh My Pi commit. See [profile provenance](profile/UPSTREAM.md) for the import changes. | MIT. [Profile license](profile/LICENSE). |
| The Magic line edit compiler, views, and prompt templates in `magic/` | Oh My Pi v19.1.0. See [Magic provenance](magic/UPSTREAM.md) for paths and adaptations. | [Upstream MIT license](https://github.com/can1357/oh-my-pi/blob/a7135b8f31826c878734388b648f52bed8da436e/LICENSE). |
| `crates/vendor/grep-searcher` | [ripgrep `grep-searcher` 0.1.17](https://crates.io/crates/grep-searcher/0.1.17), via the Oh My Pi vendor tree. | MIT or Unlicense. [MIT license](crates/vendor/grep-searcher/LICENSE-MIT) and [Unlicense](crates/vendor/grep-searcher/UNLICENSE). |
| `crates/vendor/tree-sitter-go` | [tree-sitter-go 0.25.0](https://github.com/tree-sitter/tree-sitter-go/tree/v0.25.0), via the Oh My Pi vendor tree. | MIT. [Go grammar license](crates/vendor/tree-sitter-go/LICENSE). |
| `crates/ast/src/language/testdata/ini.wasm` | [`tree-sitter-ini` 1.4.0](https://crates.io/crates/tree-sitter-ini/1.4.0) by Justin M. Keyes and contributors, compiled by [`stencil-hq/wasm-grammars` v1](https://github.com/stencil-hq/wasm-grammars/releases/tag/v1). The fixture matches the uncompressed release asset. | Apache-2.0. [INI grammar license](licenses/tree-sitter-ini-1.4.0-APACHE-2.0.txt). |

The INI fixture has SHA-256 `4884a74e0e60d02fc897b1f780c044e578ff839c9eeb6f4742d2feaef9839ece`, as recorded in the [WASM release manifest](https://github.com/stencil-hq/wasm-grammars/releases/download/v1/manifest.json). The grammar's published crate identifies source revision [`d1f6ae18e86de3c21bb6ab634ff9ab549ceb1249`](https://github.com/justinmk/tree-sitter-ini/tree/d1f6ae18e86de3c21bb6ab634ff9ab549ceb1249). The license copy above comes from that crate.

Selected MIT editing, path, and traversal fixes come from v19.1.0. The [AST provenance](crates/ast/UPSTREAM.md), [editing provenance](crates/edit/UPSTREAM.md), and [filesystem provenance](crates/xfs/UPSTREAM.md) identify the exact commits and files.

The source distribution includes the toolkit and the vendored files listed above. Source builds obtain the Shell and `obash` engines from [Oh My Pi v19.1.0 (`a7135b8`)](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e).

Binary bundles contain the compiled Rust tools, the upstream Shell and `obash` engines, and the toolkit's Node adapters. Open `runtime/licenses/BINARY_DEPENDENCIES.md` in the archive, or `licenses/BINARY_DEPENDENCIES.md` beside the installed runtime, for the target's Cargo package notices and Rust Standard Library attribution. The bundle retains the pinned Shell's license and [RTK notice](https://github.com/can1357/oh-my-pi/blob/a7135b8f31826c878734388b648f52bed8da436e/crates/shell/NOTICE). It provides the unchanged source archive of each MPL-covered dependency, including `uluru`, alongside its license.

For the macOS binary bundles, the Shell build uses the system `open` command by omitting the upstream `util.open` builtin and its SDK-derived framework bindings.

The installer obtains Node.js and the common language servers from their upstream distributors. Language servers and debug adapters carry their own terms.

The toolkit is independently maintained. References to Oh My Pi and Codex identify upstream software and integration targets.
