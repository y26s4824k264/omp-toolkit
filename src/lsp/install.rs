//! Cross-platform automatic language server provisioning for `omp-toolkit`.
//!
//! Installs the full mainstream language-server bundle into an isolated managed
//! directory (`~/.local/share/omp-toolkit/lsp` on Unix,
//! `%LOCALAPPDATA%\OMP Toolkit\lsp` on Windows) without polluting global npm
//! prefixes, and automatically provisions native ecosystem servers (`gopls`,
//! `rust-analyzer`, `ruff`) whenever their host toolchain is present.

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{diff_util::write_atomic, lsp::config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerKind {
    Npm,
    Go,
    Rustup,
    UvTool,
}

impl InstallerKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Go => "go",
            Self::Rustup => "rustup",
            Self::UvTool => "uv",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ManagedServerSpec {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub command: &'static str,
    pub languages: &'static str,
    pub installer: InstallerKind,
    /// Package specs passed to the installer (e.g. npm packages or Go module path).
    pub packages: &'static [&'static str],
}

/// Complete default language-server bundle: supersedes the legacy 15-package npm set
/// and adds native ecosystem provisioning for Go, Rust, Python (Pyright + Ruff), and PHP.
pub const MANAGED_SERVERS: &[ManagedServerSpec] = &[
    ManagedServerSpec {
        id: "typescript",
        aliases: &["ts", "js", "javascript", "tsx", "jsx"],
        command: "typescript-language-server",
        languages: "TypeScript / JavaScript",
        installer: InstallerKind::Npm,
        packages: &["typescript", "typescript-language-server"],
    },
    ManagedServerSpec {
        id: "vue",
        aliases: &["volar"],
        command: "vue-language-server",
        languages: "Vue 3 (Volar)",
        installer: InstallerKind::Npm,
        packages: &["@vue/language-server"],
    },
    ManagedServerSpec {
        id: "web",
        aliases: &["html", "css", "json", "eslint"],
        command: "vscode-json-language-server",
        languages: "HTML / CSS / JSON / ESLint",
        installer: InstallerKind::Npm,
        packages: &["vscode-langservers-extracted"],
    },
    ManagedServerSpec {
        id: "pyright",
        aliases: &["python", "py"],
        command: "pyright-langserver",
        languages: "Python (Pyright)",
        installer: InstallerKind::Npm,
        packages: &["pyright"],
    },
    ManagedServerSpec {
        id: "yaml",
        aliases: &["yml"],
        command: "yaml-language-server",
        languages: "YAML",
        installer: InstallerKind::Npm,
        packages: &["yaml-language-server"],
    },
    ManagedServerSpec {
        id: "bash",
        aliases: &["sh", "shell", "zsh"],
        command: "bash-language-server",
        languages: "Bash / Shell",
        installer: InstallerKind::Npm,
        packages: &["bash-language-server"],
    },
    ManagedServerSpec {
        id: "dockerfile",
        aliases: &["docker"],
        command: "docker-langserver",
        languages: "Dockerfile",
        installer: InstallerKind::Npm,
        packages: &["dockerfile-language-server-nodejs"],
    },
    ManagedServerSpec {
        id: "tailwindcss",
        aliases: &["tailwind"],
        command: "tailwindcss-language-server",
        languages: "Tailwind CSS",
        installer: InstallerKind::Npm,
        packages: &["@tailwindcss/language-server"],
    },
    ManagedServerSpec {
        id: "svelte",
        aliases: &[],
        command: "svelteserver",
        languages: "Svelte",
        installer: InstallerKind::Npm,
        packages: &["svelte-language-server"],
    },
    ManagedServerSpec {
        id: "astro",
        aliases: &[],
        command: "astro-ls",
        languages: "Astro",
        installer: InstallerKind::Npm,
        packages: &["@astrojs/language-server"],
    },
    ManagedServerSpec {
        id: "graphql",
        aliases: &["gql"],
        command: "graphql-lsp",
        languages: "GraphQL",
        installer: InstallerKind::Npm,
        packages: &["graphql", "graphql-language-service-cli"],
    },
    ManagedServerSpec {
        id: "prisma",
        aliases: &[],
        command: "prisma-language-server",
        languages: "Prisma",
        installer: InstallerKind::Npm,
        packages: &["@prisma/language-server"],
    },
    ManagedServerSpec {
        id: "emmet",
        aliases: &[],
        command: "emmet-language-server",
        languages: "Emmet",
        installer: InstallerKind::Npm,
        packages: &["emmet-language-server"],
    },
    ManagedServerSpec {
        id: "vim",
        aliases: &["vimscript"],
        command: "vim-language-server",
        languages: "Vimscript",
        installer: InstallerKind::Npm,
        packages: &["vim-language-server"],
    },
    ManagedServerSpec {
        id: "php",
        aliases: &["intelephense"],
        command: "intelephense",
        languages: "PHP (Intelephense)",
        installer: InstallerKind::Npm,
        packages: &["intelephense"],
    },
    ManagedServerSpec {
        id: "gopls",
        aliases: &["go", "golang"],
        command: "gopls",
        languages: "Go (gopls)",
        installer: InstallerKind::Go,
        packages: &["golang.org/x/tools/gopls@latest"],
    },
    ManagedServerSpec {
        id: "rust-analyzer",
        aliases: &["rust", "rs"],
        command: "rust-analyzer",
        languages: "Rust (rust-analyzer)",
        installer: InstallerKind::Rustup,
        packages: &["rust-analyzer"],
    },
    ManagedServerSpec {
        id: "ruff",
        aliases: &["python-lint"],
        command: "ruff",
        languages: "Python Linter (Ruff)",
        installer: InstallerKind::UvTool,
        packages: &["ruff"],
    },
];

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Optional explicit subset of server IDs / aliases to install (empty = full default bundle).
    pub only: Vec<String>,
    /// Preview actions without running installers or writing files.
    pub dry_run: bool,
    /// Reinstall/upgrade even if the command is already available on PATH or in the managed dir.
    pub force: bool,
    /// Optional override for the managed LSP root directory (mainly for tests).
    pub managed_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInstallStatus {
    pub id: String,
    pub command: String,
    pub languages: String,
    pub installer: String,
    pub available: bool,
    pub resolved_binary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallStepReport {
    pub group: String,
    pub installer: String,
    pub status: String,
    pub servers: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallSummary {
    pub managed_dir: String,
    pub dry_run: bool,
    pub steps: Vec<InstallStepReport>,
    pub statuses: Vec<ServerInstallStatus>,
}

/// Default isolated directory where `omp-toolkit` stores managed LSP packages.
pub fn managed_lsp_root() -> Option<PathBuf> {
    if let Some(custom) = env::var_os("OMP_TOOLKIT_LSP_DIR").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(custom));
    }
    #[cfg(target_os = "windows")]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("USERPROFILE")
                    .map(|h| PathBuf::from(h).join("AppData").join("Local"))
            })
            .map(|base| base.join("OMP Toolkit").join("lsp"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("omp-toolkit")
                .join("lsp")
        })
    }
}

/// Extra bin directories searched automatically by `omp-toolkit` after project `node_modules/.bin`
/// and system `PATH` so managed and standard toolchain language servers are always discoverable.
pub fn managed_lsp_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(root) = managed_lsp_root() {
        dirs.push(root.join("npm").join("node_modules").join(".bin"));
        dirs.push(root.join("bin"));
    }
    if let Some(home) = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        dirs.push(home.join("go").join("bin"));
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join(".local").join("bin"));
    }
    dirs
}

/// Inspect availability of every server in the managed bundle.
pub fn list_status(managed_dir_override: Option<&Path>) -> Vec<ServerInstallStatus> {
    let extra_bin = managed_dir_override.map(|d| d.join("npm").join("node_modules").join(".bin"));
    MANAGED_SERVERS
        .iter()
        .map(|spec| {
            let resolved = resolve_with_extra(spec.command, extra_bin.as_deref());
            ServerInstallStatus {
                id: spec.id.to_owned(),
                command: spec.command.to_owned(),
                languages: spec.languages.to_owned(),
                installer: spec.installer.as_str().to_owned(),
                available: resolved.is_some(),
                resolved_binary: resolved.map(|p| p.to_string_lossy().into_owned()),
            }
        })
        .collect()
}

/// Select matching server specs from `--only` tokens (or return the full bundle when empty).
pub fn select_specs(only: &[String]) -> Result<Vec<&'static ManagedServerSpec>> {
    if only.is_empty() {
        return Ok(MANAGED_SERVERS.iter().collect());
    }
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in only {
        for token in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let lower = token.to_ascii_lowercase();
            if matches!(lower.as_str(), "all" | "full" | "default") {
                for spec in MANAGED_SERVERS {
                    if seen.insert(spec.id) {
                        selected.push(spec);
                    }
                }
                continue;
            }
            let Some(spec) = MANAGED_SERVERS.iter().find(|s| {
                s.id.eq_ignore_ascii_case(&lower)
                    || s.command.eq_ignore_ascii_case(&lower)
                    || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(&lower))
            }) else {
                let valid: Vec<&str> = MANAGED_SERVERS.iter().map(|s| s.id).collect();
                bail!(
                    "unknown LSP target '{token}'; available: all, {}",
                    valid.join(", ")
                );
            };
            if seen.insert(spec.id) {
                selected.push(spec);
            }
        }
    }
    Ok(selected)
}

/// Provision missing (or all, with `force`) language servers from the selected bundle.
pub fn install_servers(opts: &InstallOptions) -> Result<InstallSummary> {
    let managed_dir = opts
        .managed_dir
        .clone()
        .or_else(managed_lsp_root)
        .context("could not determine managed LSP directory")?;
    let specs = select_specs(&opts.only)?;
    let extra_bin = managed_dir.join("npm").join("node_modules").join(".bin");

    let mut npm_specs = Vec::new();
    let mut native_specs = Vec::new();
    for spec in specs {
        let already = resolve_with_extra(spec.command, Some(&extra_bin)).is_some();
        if already && !opts.force {
            continue;
        }
        match spec.installer {
            InstallerKind::Npm => npm_specs.push(spec),
            _ => native_specs.push(spec),
        }
    }

    let mut steps = Vec::new();

    // 1. Batch-install all missing npm language servers into the isolated npm/ folder.
    if !npm_specs.is_empty() {
        steps.push(install_npm_batch(&managed_dir, &npm_specs, opts.dry_run)?);
    } else if opts.only.is_empty() {
        steps.push(InstallStepReport {
            group: "npm-bundle".into(),
            installer: "npm".into(),
            status: "already_installed".into(),
            servers: Vec::new(),
            message: "All managed npm language servers are already available.".into(),
        });
    }

    // 2. Provision native ecosystem servers when their host toolchain is installed.
    for spec in native_specs {
        steps.push(install_native_server(spec, opts.dry_run));
    }

    let statuses = list_status(Some(&managed_dir));
    Ok(InstallSummary {
        managed_dir: managed_dir.display().to_string(),
        dry_run: opts.dry_run,
        steps,
        statuses,
    })
}

fn install_npm_batch(
    managed_dir: &Path,
    specs: &[&'static ManagedServerSpec],
    dry_run: bool,
) -> Result<InstallStepReport> {
    let server_ids: Vec<String> = specs.iter().map(|s| s.id.to_owned()).collect();
    let mut pkgs: Vec<&str> = Vec::new();
    let mut seen_pkg = BTreeSet::new();
    for spec in specs {
        for &pkg in spec.packages {
            if seen_pkg.insert(pkg) {
                pkgs.push(pkg);
            }
        }
    }

    let Some(npm_bin) = config::resolve_command("npm") else {
        return Ok(InstallStepReport {
            group: "npm-bundle".into(),
            installer: "npm".into(),
            status: "skipped_missing_toolchain".into(),
            servers: server_ids,
            message: format!(
                "`npm` not found on PATH; install Node.js/npm to auto-provision: {}",
                pkgs.join(", ")
            ),
        });
    };

    let npm_dir = managed_dir.join("npm");
    if dry_run {
        return Ok(InstallStepReport {
            group: "npm-bundle".into(),
            installer: npm_bin.display().to_string(),
            status: "dry_run".into(),
            servers: server_ids,
            message: format!(
                "Would install {} npm package(s) into {}: {}",
                pkgs.len(),
                npm_dir.display(),
                pkgs.join(" ")
            ),
        });
    }

    fs::create_dir_all(&npm_dir)
        .with_context(|| format!("create managed npm directory {}", npm_dir.display()))?;
    ensure_managed_package_json(&npm_dir)?;

    let mut cmd = Command::new(&npm_bin);
    cmd.current_dir(&npm_dir)
        .args(["install", "--no-audit", "--no-fund", "--omit=dev"])
        .args(&pkgs);
    let output = cmd
        .output()
        .with_context(|| format!("execute `{}` in {}", npm_bin.display(), npm_dir.display()))?;

    if output.status.success() {
        Ok(InstallStepReport {
            group: "npm-bundle".into(),
            installer: npm_bin.display().to_string(),
            status: "installed".into(),
            servers: server_ids,
            message: format!(
                "Installed {} package(s) into {}",
                pkgs.len(),
                npm_dir.display()
            ),
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        Ok(InstallStepReport {
            group: "npm-bundle".into(),
            installer: npm_bin.display().to_string(),
            status: "failed".into(),
            servers: server_ids,
            message: format!("npm install failed: {tail}"),
        })
    }
}

fn ensure_managed_package_json(npm_dir: &Path) -> Result<()> {
    let pkg_path = npm_dir.join("package.json");
    if pkg_path.is_file() {
        return Ok(());
    }
    let body = serde_json::to_string_pretty(&json!({
        "name": "omp-toolkit-managed-lsp",
        "private": true,
        "description": "Isolated language server bundle managed by omp-toolkit",
        "dependencies": Map::new()
    }))? + "\n";
    write_atomic(&pkg_path, body.as_bytes())
}

fn install_native_server(spec: &ManagedServerSpec, dry_run: bool) -> InstallStepReport {
    let (tool_name, args): (&str, Vec<&str>) = match spec.installer {
        InstallerKind::Go => ("go", vec!["install", spec.packages[0]]),
        InstallerKind::Rustup => ("rustup", vec!["component", "add", spec.packages[0]]),
        InstallerKind::UvTool => ("uv", vec!["tool", "install", spec.packages[0]]),
        InstallerKind::Npm => unreachable!(),
    };

    let Some(tool_bin) = config::resolve_command(tool_name) else {
        return InstallStepReport {
            group: spec.id.to_owned(),
            installer: tool_name.to_owned(),
            status: "skipped_missing_toolchain".into(),
            servers: vec![spec.id.to_owned()],
            message: format!(
                "`{tool_name}` not found on PATH; skipped installing `{}`.",
                spec.command
            ),
        };
    };

    if dry_run {
        return InstallStepReport {
            group: spec.id.to_owned(),
            installer: tool_bin.display().to_string(),
            status: "dry_run".into(),
            servers: vec![spec.id.to_owned()],
            message: format!("Would run: {tool_name} {}", args.join(" ")),
        };
    }

    match Command::new(&tool_bin).args(&args).output() {
        Ok(out) if out.status.success() => InstallStepReport {
            group: spec.id.to_owned(),
            installer: tool_bin.display().to_string(),
            status: "installed".into(),
            servers: vec![spec.id.to_owned()],
            message: format!("Installed `{}` via `{tool_name}`.", spec.command),
        },
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            InstallStepReport {
                group: spec.id.to_owned(),
                installer: tool_bin.display().to_string(),
                status: "failed".into(),
                servers: vec![spec.id.to_owned()],
                message: format!("{tool_name} {} failed: {err}", args.join(" ")),
            }
        }
        Err(e) => InstallStepReport {
            group: spec.id.to_owned(),
            installer: tool_bin.display().to_string(),
            status: "failed".into(),
            servers: vec![spec.id.to_owned()],
            message: format!("failed to launch `{tool_name}`: {e}"),
        },
    }
}

fn resolve_with_extra(command: &str, extra_bin: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = extra_bin
        && let Some(found) = config::find_in_dir(dir, command)
    {
        return Some(found);
    }
    config::resolve_command(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_full_bundle_and_aliases() -> Result<()> {
        let all = select_specs(&[])?;
        assert!(all.len() >= 15);
        assert!(all.iter().any(|s| s.id == "typescript"));
        assert!(all.iter().any(|s| s.id == "vue"));
        assert!(all.iter().any(|s| s.id == "gopls"));
        assert!(all.iter().any(|s| s.id == "rust-analyzer"));

        let subset = select_specs(&["ts,vue,go".into(), "py".into()])?;
        let ids: Vec<&str> = subset.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["typescript", "vue", "gopls", "pyright"]);
        Ok(())
    }

    #[test]
    fn dry_run_install_never_touches_disk() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let managed = tmp.path().join("lsp");
        let summary = install_servers(&InstallOptions {
            only: vec!["typescript".into(), "vue".into()],
            dry_run: true,
            force: true,
            managed_dir: Some(managed.clone()),
        })?;
        assert!(summary.dry_run);
        assert!(!managed.exists());
        assert!(!summary.steps.is_empty());
        Ok(())
    }
}
