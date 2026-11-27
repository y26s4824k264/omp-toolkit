#!/usr/bin/env bash
set -euo pipefail

# One-command source build, install, and AI client adapter for OMP Toolkit.
# Zero Node.js dependency, zero global prompt-file pollution.

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
bin_dir="${OMP_TOOLKIT_BIN_DIR:-$HOME/.local/bin}"
clients=()
dry_run=false
force=false
remove=false
skip_build=false
no_path_env=false
without_lsp=false

usage() {
    cat <<'EOF'
Usage: ./install.sh [options]

Build `omp-toolkit`, install the single binary, auto-provision the managed language server suite,
and optionally wire it into your AI coding software.

Options:
  --bin-dir DIR         Install directory for `omp-toolkit` (default: ~/.local/bin)
  --client NAME         Client to configure (repeatable):
                          none (install binary only, default)
                          list (inspect detected clients without changing anything)
                          auto (configure all detected clients)
                          antigravity | antigravity-mcp | claude-code | codex |
                          cursor | vscode | claude-desktop | windsurf | zed |
                          workbuddy | codebuddy | trae | qoder | lingma |
                          kiro | cline | roo-code
  --without-lsp         Skip automatic language server provisioning (`omp-toolkit lsp-install`)
  --dry-run             Preview setup/remove actions without modifying client configs
  --remove              Remove `omp` integration from selected `--client` target(s)
  --force               Replace an existing `omp` entry even if it points elsewhere
  --no-path-env         Do not embed current PATH into GUI MCP configs
  --skip-build          Reuse existing target/release/omp-toolkit instead of running cargo build
  -h, --help            Show this help

Examples:
  ./install.sh --client antigravity
  ./install.sh --client workbuddy --client trae --client qoder
  ./install.sh --client auto --dry-run
  ./install.sh --client antigravity --client claude-code --client cursor
  ./install.sh --skip-build --client antigravity --remove
EOF
}

die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin-dir)
            [[ $# -ge 2 && -n "$2" ]] || die '--bin-dir requires a directory'
            bin_dir=$2
            shift 2
            ;;
        --client)
            [[ $# -ge 2 && -n "$2" ]] || die '--client requires a value'
            clients+=("$2")
            shift 2
            ;;
        --dry-run) dry_run=true; shift ;;
        --force) force=true; shift ;;
        --remove) remove=true; shift ;;
        --skip-build) skip_build=true; shift ;;
        --no-path-env) no_path_env=true; shift ;;
        --without-lsp) without_lsp=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

release_bin="$repo_dir/target/release/omp-toolkit"

if ! "$skip_build"; then
    command -v cargo >/dev/null || die 'cargo is required to build from source'
    (
        cd "$repo_dir"
        cargo build --release --bin omp-toolkit
    )
fi

[[ -x "$release_bin" ]] || die "binary not found at $release_bin (omit --skip-build to compile)"

if "$remove"; then
    installed_bin="$release_bin"
    [[ -x "$bin_dir/omp-toolkit" ]] && installed_bin="$bin_dir/omp-toolkit"
else
    if "$dry_run"; then
        installed_bin="$bin_dir/omp-toolkit"
        printf '[dry-run] Would install %s -> %s\n' "$release_bin" "$installed_bin"
    else
        mkdir -p "$bin_dir"
        tmp_bin="$bin_dir/.omp-toolkit.tmp.$$"
        cp "$release_bin" "$tmp_bin"
        chmod 755 "$tmp_bin"
        mv -f "$tmp_bin" "$bin_dir/omp-toolkit"
        installed_bin="$bin_dir/omp-toolkit"
        printf 'Installed binary: %s\n' "$installed_bin"
    fi

    if ! "$without_lsp"; then
        lsp_runner="$installed_bin"
        lsp_args=(lsp-install)
        if "$dry_run"; then
            lsp_args+=(--dry-run)
            [[ -x "$lsp_runner" ]] || lsp_runner="$release_bin"
        fi
        printf '\nProvisioning managed language servers (pass --without-lsp to skip)...\n'
        "$lsp_runner" "${lsp_args[@]}" || printf 'Warning: some language servers could not be installed; re-run `%s lsp-install` later.\n' "$installed_bin" >&2
    fi
fi

if [[ ${#clients[@]} -eq 0 ]]; then
    clients=("none")
fi

# Skip setup when --client none
if [[ ${#clients[@]} -eq 1 && "${clients[0]}" == "none" ]]; then
    printf '\nBinary ready. Run `%s setup --client list` to inspect supported AI clients,\n' "$installed_bin"
    printf 'or `%s setup --client antigravity` (or `auto`) to connect.\n' "$installed_bin"
    exit 0
fi

setup_args=(setup)
for c in "${clients[@]}"; do
    setup_args+=(--client "$c")
done
if "$dry_run"; then setup_args+=(--dry-run); fi
if "$force"; then setup_args+=(--force); fi
if "$remove"; then setup_args+=(--remove); fi
if "$no_path_env"; then setup_args+=(--no-path-env); fi

runner="$installed_bin"
if "$dry_run" && [[ ! -x "$runner" ]]; then
    runner="$release_bin"
    setup_args+=(--install-bin "$bin_dir")
fi

"$runner" "${setup_args[@]}"
