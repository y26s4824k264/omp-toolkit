//! Lean, native extraction of Oh My Pi's highest-value code intelligence engines:
//! - Tree-sitter structural reading & syntactic block expansion (`code_summary`, `code_outline`)
//! - Full-repository parallel AST search & rewrite (`ast_grep`, `ast_edit`)
//! - Persistent cross-platform LSP connection pool & semantic rename (`lsp`)
//! - Persistent cross-platform DAP debugger (`debug`)
//! - Standalone command output minimizer (`minimize_output`)
//! - Full-repository Git merge conflict scanner & structured resolver (`git_conflicts`)

pub mod code_ast;
pub mod conflicts;
pub mod dap;
pub mod diff_util;
pub mod lsp;
pub mod mcp;
pub mod minimize;
pub mod path_guard;
pub mod setup;
