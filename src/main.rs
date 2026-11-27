use std::{
    io::{self, Read},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use omp_toolkit::{
    code_ast::{self, AstEditInput, AstGrepInput, OutlineInput, RewriteRule, SummaryInput},
    conflicts::{self, ConflictsInput},
    dap::{self, DebugInput, session::DapManager},
    lsp::{self, LspInput, pool::LspPool},
    mcp,
    minimize::{self, MinimizeInput},
    setup::{self, Client, SetupOptions},
};

#[derive(Parser)]
#[command(
    name = "omp-toolkit",
    version,
    about = "Lean native MCP server & CLI extracting Oh My Pi's core code intelligence (AST, LSP, DAP, conflicts, output minimizer)"
)]
struct Cli {
    /// Project root directory (defaults to current working directory).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the persistent stdio MCP server (recommended for coding agents).
    Mcp,
    /// Fold a source file into an AST-aware structural summary, or expand a single syntactic block.
    Summary {
        file: PathBuf,
        #[arg(long)]
        lang: Option<String>,
        /// Expand the full syntactic block at this 1-based line (including attributes/decorators).
        #[arg(long)]
        block_line: Option<u32>,
        #[arg(long)]
        unfold_until: Option<u32>,
        #[arg(long)]
        unfold_limit: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// List top-level symbols and members of a source file using tree-sitter.
    Outline {
        file: PathBuf,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long = "type")]
        types: Vec<String>,
        #[arg(long)]
        include_imports: bool,
    },
    /// Full-repository parallel AST structural search.
    AstGrep {
        /// AST pattern(s) to match.
        #[arg(long = "pattern", required = true)]
        patterns: Vec<String>,
        /// Optional subdirectory or file inside `--root`.
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        include: Option<String>,
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        #[arg(long)]
        strictness: Option<String>,
        #[arg(long)]
        hidden: bool,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Full-repository parallel AST structural rewrite (preview diff by default; pass `--apply` to write).
    AstEdit {
        #[arg(long)]
        pattern: String,
        #[arg(long)]
        rewrite: String,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        include: Option<String>,
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        #[arg(long)]
        strictness: Option<String>,
        #[arg(long)]
        skip_overlaps: bool,
        #[arg(long)]
        apply: bool,
    },
    /// Run a single LSP action (for warm reuse across turns, use `omp-toolkit mcp`).
    Lsp {
        action: String,
        file: Option<PathBuf>,
        #[arg(long)]
        line: Option<u32>,
        #[arg(long)]
        column: Option<u32>,
        #[arg(long)]
        symbol: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        new_name: Option<String>,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Auto-install and verify the managed language server suite (TypeScript, Vue, Web/ESLint,
    /// Python/Pyright, YAML, Bash, Docker, Tailwind, Svelte, Astro, GraphQL, Prisma, PHP, Go, Rust, Ruff).
    LspInstall {
        /// Optional comma-separated or repeated targets (e.g. `--only ts,vue,go,rust`); omit for the full suite.
        #[arg(long = "only")]
        only: Vec<String>,
        /// List installation status of all managed language servers without installing anything.
        #[arg(long)]
        list: bool,
        /// Preview install actions without running package managers or modifying disk.
        #[arg(long)]
        dry_run: bool,
        /// Reinstall/upgrade servers even if already found on PATH.
        #[arg(long)]
        force: bool,
        /// Output machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect available DAP debug adapters (live multi-step sessions run over `omp-toolkit mcp`).
    DebugAdapters,
    /// Compress noisy CLI output from stdin or a file using OMP's command-aware minimizer.
    Minimize {
        /// Original command line (e.g. "cargo test", "git diff", "pytest").
        #[arg(long = "cmd")]
        command: String,
        #[arg(long, default_value_t = 0)]
        exit_code: i32,
        /// Optional log file inside `--root`; reads stdin when omitted.
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        min_chars: usize,
        #[arg(long)]
        json: bool,
    },
    /// Scan or resolve Git merge conflicts across a file or repository.
    Conflicts {
        /// `scan` or `resolve`.
        action: String,
        /// Optional file or subdirectory inside `--root` (required file for `resolve`).
        file: Option<PathBuf>,
        /// Optional include glob when scanning.
        #[arg(long)]
        include: Option<String>,
        /// Optional exclude glob(s) when scanning.
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        /// 1-based conflict index to resolve.
        #[arg(long)]
        index: Option<usize>,
        /// Apply `--choice` to all conflict blocks in `file`.
        #[arg(long)]
        all: bool,
        /// Resolution strategy: ours | theirs | base | both | both_reverse | delete | custom.
        #[arg(long)]
        choice: Option<String>,
        /// Replacement text when `--choice custom` (supports @ours/@theirs/@base/@both lines).
        #[arg(long)]
        custom_text: Option<String>,
        /// Write resolved file atomically (defaults to unified diff preview).
        #[arg(long)]
        apply: bool,
    },
    /// List supported built-in AST languages.
    Languages,
    /// Detect, configure, preview, or remove integrations for supported AI coding clients.
    Setup {
        /// Client(s) to configure: `auto`, `list`, `antigravity`, `antigravity-mcp`,
        /// `claude-code`, `codex`, `cursor`, `vscode`, `claude-desktop`, `windsurf`, `zed`,
        /// `workbuddy`, `codebuddy`, `trae`, `qoder`, `lingma`, `kiro`, `cline`, `roo-code`.
        #[arg(long = "client", default_value = "list")]
        clients: Vec<String>,
        /// Print ready-to-paste config snippet for one client without modifying disk.
        #[arg(long)]
        print: Option<String>,
        /// Copy this binary into a stable bin directory (e.g. ~/.local/bin) before registering.
        #[arg(long)]
        install_bin: Option<PathBuf>,
        /// Remove the `omp` integration from the selected client(s).
        #[arg(long)]
        remove: bool,
        /// Preview actions without modifying any files or invoking external CLIs.
        #[arg(long)]
        dry_run: bool,
        /// Replace an existing `omp` entry even if it points to another binary.
        #[arg(long)]
        force: bool,
        /// Omit embedding current PATH into GUI MCP config `env.PATH`.
        #[arg(long)]
        no_path_env: bool,
        /// Override home directory (advanced / testing).
        #[arg(long, hide = true)]
        home: Option<PathBuf>,
        /// Output machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = match cli.root {
        Some(r) => r,
        None => std::env::current_dir().context("resolve current directory")?,
    };

    match cli.command {
        Command::Mcp => mcp::serve_stdio().await,
        Command::Languages => {
            println!("{}", code_ast::supported_languages());
            Ok(())
        }
        Command::Setup {
            clients,
            print,
            install_bin,
            remove,
            dry_run,
            force,
            no_path_env,
            home,
            json,
        } => {
            let current_exe = std::env::current_exe()
                .context("resolve current executable")?
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_exe().unwrap());
            let binary = match install_bin {
                Some(dir) => setup::install_binary(&current_exe, &dir, dry_run)?,
                None => current_exe,
            };
            let opts = SetupOptions {
                binary,
                home: home.clone(),
                path_env: None,
                dry_run,
                force,
                include_path_env: !no_path_env,
            };

            if let Some(target) = print {
                let c = Client::parse(&target)?;
                println!("{}", setup::render_snippet(c, &opts)?);
                return Ok(());
            }

            if clients.len() == 1 && clients[0].eq_ignore_ascii_case("list") {
                let detected = setup::detect_clients(home.as_deref())?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&detected)?);
                } else {
                    println!("{:<18} {:<10} {}", "CLIENT", "DETECTED", "TARGET");
                    for row in detected {
                        println!(
                            "{:<18} {:<10} {}",
                            row.client,
                            if row.detected { "yes" } else { "no" },
                            row.target
                        );
                    }
                    println!("\nRun `omp-toolkit setup --client auto --dry-run` to preview, or `--client antigravity` to configure one.");
                }
                return Ok(());
            }

            let mut resolved = Vec::new();
            for raw in &clients {
                if raw.eq_ignore_ascii_case("auto") {
                    for c in setup::resolve_auto_clients(home.as_deref())? {
                        if !resolved.contains(&c) {
                            resolved.push(c);
                        }
                    }
                } else {
                    let c = Client::parse(raw)?;
                    if !resolved.contains(&c) {
                        resolved.push(c);
                    }
                }
            }

            if resolved.is_empty() {
                if json {
                    println!("[]");
                } else {
                    println!("No supported clients auto-detected. Specify `--client <name>` or `--print <name>` (see `omp-toolkit setup --client list`).");
                }
                return Ok(());
            }

            let reports = if remove {
                setup::remove_clients(&resolved, &opts)?
            } else {
                setup::apply_clients(&resolved, &opts)?
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            } else {
                for r in reports {
                    println!("[{}] {} -> {}\n    {}", r.status, r.display_name, r.target, r.message);
                }
            }
            Ok(())
        }
        Command::Summary {
            file,
            lang,
            block_line,
            unfold_until,
            unfold_limit,
            json,
        } => {
            let out = code_ast::code_summary(SummaryInput {
                root,
                file,
                lang,
                block_line,
                unfold_until_lines: unfold_until,
                unfold_limit_lines: unfold_limit,
                min_body_lines: None,
                min_comment_lines: None,
                compact: Some(!json),
            })?;
            print!("{out}");
            Ok(())
        }
        Command::Conflicts {
            action,
            file,
            include,
            exclude,
            index,
            all,
            choice,
            custom_text,
            apply,
        } => {
            let val = conflicts::execute(ConflictsInput {
                action,
                root,
                file,
                include,
                exclude: (!exclude.is_empty()).then_some(exclude),
                hidden: None,
                max_conflicts: None,
                index,
                all: Some(all),
                choice,
                custom_text,
                resolutions: None,
                apply: Some(apply),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&val)?);
            Ok(())
        }
        Command::Outline {
            file,
            lang,
            types,
            include_imports,
        } => {
            let val = code_ast::code_outline(OutlineInput {
                root,
                file,
                lang,
                types: (!types.is_empty()).then_some(types),
                include_imports: Some(include_imports),
            })?;
            println!("{}", serde_json::to_string_pretty(&val)?);
            Ok(())
        }
        Command::AstGrep {
            patterns,
            path,
            lang,
            selector,
            include,
            exclude,
            strictness,
            hidden,
            offset,
            limit,
        } => {
            let val = code_ast::ast_grep(AstGrepInput {
                root,
                path,
                patterns,
                lang,
                selector,
                include,
                exclude: (!exclude.is_empty()).then_some(exclude),
                strictness,
                hidden: Some(hidden),
                offset: Some(offset),
                limit: Some(limit),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&val)?);
            Ok(())
        }
        Command::AstEdit {
            pattern,
            rewrite,
            path,
            lang,
            selector,
            include,
            exclude,
            strictness,
            skip_overlaps,
            apply,
        } => {
            let val = code_ast::ast_edit(AstEditInput {
                root,
                path,
                rules: vec![RewriteRule { pattern, rewrite }],
                lang,
                selector,
                include,
                exclude: (!exclude.is_empty()).then_some(exclude),
                strictness,
                skip_overlaps: Some(skip_overlaps),
                max_replacements: None,
                max_files: None,
                apply: Some(apply),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&val)?);
            Ok(())
        }
        Command::Lsp {
            action,
            file,
            line,
            column,
            symbol,
            query,
            new_name,
            apply,
            timeout,
        } => {
            let pool = LspPool::new();
            let res = lsp::execute(
                &pool,
                LspInput {
                    action,
                    root,
                    file,
                    files: None,
                    line,
                    column,
                    symbol,
                    query,
                    new_name,
                    apply: Some(apply),
                    timeout_sec: timeout,
                },
            )
            .await;
            pool.shutdown_all().await;
            let val = res?;
            println!("{}", serde_json::to_string_pretty(&val)?);
            Ok(())
        }
        Command::LspInstall {
            only,
            list,
            dry_run,
            force,
            json,
        } => {
            if list {
                let statuses = lsp::install::list_status(None);
                if json {
                    println!("{}", serde_json::to_string_pretty(&statuses)?);
                } else {
                    println!("{:<16} {:<10} {:<10} {}", "SERVER", "STATUS", "VIA", "LANGUAGES / BINARY");
                    for s in statuses {
                        let detail = s
                            .resolved_binary
                            .unwrap_or_else(|| format!("({})", s.languages));
                        println!(
                            "{:<16} {:<10} {:<10} {}",
                            s.id,
                            if s.available { "ready" } else { "missing" },
                            s.installer,
                            detail
                        );
                    }
                }
                return Ok(());
            }
            let summary = lsp::install::install_servers(&lsp::install::InstallOptions {
                only,
                dry_run,
                force,
                managed_dir: None,
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!("Managed LSP directory: {}", summary.managed_dir);
                for step in &summary.steps {
                    println!("[{}] {} ({})\n    {}", step.status, step.group, step.installer, step.message);
                }
                let ready = summary.statuses.iter().filter(|s| s.available).count();
                println!(
                    "\nLanguage servers ready: {ready}/{} (run `omp-toolkit lsp-install --list` for details)",
                    summary.statuses.len()
                );
            }
            Ok(())
        }
        Command::DebugAdapters => {
            let mgr = DapManager::new();
            let val = dap::execute(
                &mgr,
                DebugInput {
                    action: "adapters".into(),
                    root: Some(root),
                    session_id: None,
                    program: None,
                    args: None,
                    adapter: None,
                    stop_on_entry: None,
                    pid: None,
                    host: None,
                    port: None,
                    file: None,
                    line: None,
                    condition: None,
                    hit_condition: None,
                    levels: None,
                    frame_id: None,
                    variable_ref: None,
                    expression: None,
                    context: None,
                    clear: None,
                    timeout_sec: None,
                },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&val)?);
            Ok(())
        }
        Command::Minimize {
            command,
            exit_code,
            file,
            min_chars,
            json,
        } => {
            let input = if let Some(f) = file {
                MinimizeInput {
                    command,
                    exit_code: Some(exit_code),
                    output: None,
                    root: Some(root),
                    file: Some(f),
                    min_chars: Some(min_chars),
                }
            } else {
                let mut raw = String::new();
                io::stdin()
                    .read_to_string(&mut raw)
                    .context("read stdin for minimize")?;
                MinimizeInput {
                    command,
                    exit_code: Some(exit_code),
                    output: Some(raw),
                    root: None,
                    file: None,
                    min_chars: Some(min_chars),
                }
            };
            let val = minimize::minimize_output(input)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
                print!("{text}");
            }
            Ok(())
        }
    }
}
