#!/usr/bin/env bash
set -euo pipefail

upstream_url=https://github.com/can1357/oh-my-pi.git
upstream_commit=57771ae562b9345e0579bceb2d68d585e3e6927e
upstream_version=19.0.0
toolkit_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
prefix="$HOME/.local/omp-codex-toolkit"
omp_bin="$HOME/.omp/bin/$upstream_version/omp"
build_dir="$toolkit_dir/target"
upstream_source=
with_lsp_servers=false

usage() {
    cat <<'EOF'
Usage: scripts/install.sh [options]

  --prefix DIR           Install into DIR (default: ~/.local/omp-codex-toolkit)
  --omp-bin FILE         OMP 19.0.0 executable (default: ~/.omp/bin/19.0.0/omp)
  --upstream-source DIR  Read the pinned Oh My Pi commit from a local Git repository
  --build-dir DIR        Store Cargo build outputs in DIR (default: toolkit target/)
  --with-lsp-servers     Install the locked Node language server dependencies
  --help                 Display this message
EOF
}

die() {
    printf 'install: %s\n' "$*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix|--omp-bin|--upstream-source|--build-dir)
            [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || die "$1 requires a path"
            case "$1" in
                --prefix) prefix=$2 ;;
                --omp-bin) omp_bin=$2 ;;
                --upstream-source) upstream_source=$2 ;;
                --build-dir) build_dir=$2 ;;
            esac
            shift 2
            ;;
        --with-lsp-servers) with_lsp_servers=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

for dependency in git cargo node; do
    command -v "$dependency" >/dev/null || die "$dependency is required"
done
if "$with_lsp_servers"; then
    command -v npm >/dev/null || die 'npm is required with --with-lsp-servers'
fi

node_major=$(node -p 'process.versions.node.split(".")[0]')
[[ "$node_major" -ge 24 ]] || die 'Node.js 24 or later is required'
node_bin=$(node -p 'process.execPath')
[[ -f "$omp_bin" && -x "$omp_bin" ]] || die "OMP executable not found: $omp_bin. Use --omp-bin FILE."
omp_dir=$(cd -- "$(dirname -- "$omp_bin")" && pwd -P)
omp_bin="$omp_dir/$(basename -- "$omp_bin")"
omp_version=$("$omp_bin" --version) || die "cannot run $omp_bin --version"
[[ "$omp_version" == "omp/$upstream_version" ]] || die "expected omp/$upstream_version, found $omp_version"

mkdir -p -- "$build_dir"
build_dir=$(cd -- "$build_dir" && pwd -P)
stage_dir=$(mktemp -d "${TMPDIR:-/tmp}/omp-codex-toolkit.XXXXXXXX")
trap 'rm -rf -- "$stage_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -z "$upstream_source" ]]; then
    upstream_source="$stage_dir/upstream-git"
    git init --quiet "$upstream_source"
    git -C "$upstream_source" fetch --quiet --depth=1 --no-tags "$upstream_url" "$upstream_commit"
fi
git -C "$upstream_source" cat-file -e "$upstream_commit^{commit}" || die "upstream source does not contain $upstream_commit"
upstream_dir="$stage_dir/upstream"
mkdir -p "$upstream_dir"
git -C "$upstream_source" archive "$upstream_commit" | tar -xf - -C "$upstream_dir"

shell_example="$upstream_dir/crates/shell/examples/omp_shell_cli"
mkdir -p "$shell_example"
cp "$toolkit_dir/shell/omp_shell_cli.rs" "$shell_example/main.rs"
cp "$toolkit_dir/src/log_preview.rs" "$shell_example/log_preview.rs"

(
    cd "$toolkit_dir"
    cargo build --locked --release -p omp-refactor --bins --target-dir "$build_dir/toolkit"
)
(
    cd "$upstream_dir"
    # Use the same nightly without installing the upstream workspace's cross targets.
    cargo +nightly-2026-11-08 build --locked --profile ci -p omp-shell \
        --example omp_shell_cli --bin obash --target-dir "$build_dir/upstream-shell"
)

payload="$stage_dir/payload"
mkdir -p "$payload/bin" "$payload/runtime/debug" "$payload/runtime/lsp/servers" \
    "$payload/runtime/profile" "$payload/runtime/stats" "$payload/runtime/licenses"
cp "$toolkit_dir/debug/"{bridge,omp-debug,protocol}.mjs "$payload/runtime/debug/"
cp "$toolkit_dir/lsp/"{bridge,omp-lsp,rename-patch,rename-preview}.mjs "$payload/runtime/lsp/"
cp "$toolkit_dir/stats/codex-context-stats.mjs" "$payload/runtime/stats/"
cp -R "$toolkit_dir/profile/." "$payload/runtime/profile/"
cp "$toolkit_dir/lsp/servers/"{package,package-lock}.json "$payload/runtime/lsp/servers/"
if "$with_lsp_servers"; then
    (
        cd -- "$payload/runtime/lsp/servers"
        npm ci --ignore-scripts --no-audit --no-fund
    )
fi

licenses="$payload/runtime/licenses"
cp "$toolkit_dir/LICENSE" "$licenses/toolkit-LICENSE"
cp "$toolkit_dir/crates/ast/LICENSE" "$licenses/omp-ast-LICENSE"
cp "$toolkit_dir/crates/xfs/LICENSE" "$licenses/omp-xfs-LICENSE"
cp "$toolkit_dir/crates/vendor/grep-searcher/LICENSE-MIT" "$licenses/grep-searcher-LICENSE-MIT"
cp "$toolkit_dir/crates/vendor/grep-searcher/UNLICENSE" "$licenses/grep-searcher-UNLICENSE"
cp "$toolkit_dir/crates/vendor/tree-sitter-go/LICENSE" "$licenses/tree-sitter-go-LICENSE"
cp "$upstream_dir/LICENSE" "$licenses/oh-my-pi-LICENSE"
cp "$upstream_dir/crates/shell/LICENSE" "$licenses/omp-shell-LICENSE"
cp "$upstream_dir/crates/shell/NOTICE" "$licenses/omp-shell-NOTICE"
if [[ -f "$toolkit_dir/THIRD_PARTY_NOTICES.md" ]]; then
    cp "$toolkit_dir/THIRD_PARTY_NOTICES.md" "$payload/runtime/"
fi
if [[ -d "$toolkit_dir/licenses" ]]; then
    cp -R "$toolkit_dir/licenses/." "$licenses/"
fi

install -m 755 "$build_dir/toolkit/release/omp-refactor" "$payload/bin/omp-refactor"
install -m 755 "$build_dir/toolkit/release/omp-codex-mcp" "$payload/bin/omp-codex-mcp"
install -m 755 "$build_dir/upstream-shell/ci/examples/omp_shell_cli" "$payload/bin/omp-shell"
install -m 755 "$build_dir/upstream-shell/ci/obash" "$payload/bin/obash"

mkdir -p -- "$prefix"
prefix=$(cd -- "$prefix" && pwd -P)
bin_dir="$prefix/bin"
runtime_dir="$prefix/share/omp-codex-toolkit"

write_launcher() {
    local launcher=$1 module=$2 omp_env=${3:-}
    {
        printf '#!/bin/bash\n'
        if [[ -n "$omp_env" ]]; then
            printf 'omp_bin=%q\n' "$omp_bin"
            # shellcheck disable=SC2016
            printf 'export %s="${%s:-$omp_bin}"\n' "$omp_env" "$omp_env"
        fi
        # shellcheck disable=SC2016
        printf 'exec %q %q "$@"\n' "$node_bin" "$runtime_dir/$module"
    } > "$payload/bin/$launcher"
}

write_launcher omp-lsp lsp/omp-lsp.mjs OMP_LSP_BIN
write_launcher omp-debug debug/omp-debug.mjs OMP_DEBUG_BIN
write_launcher codex-context-stats stats/codex-context-stats.mjs

mkdir -p -- "$bin_dir" "$runtime_dir"
cp -R "$payload/runtime/." "$runtime_dir/"
install -m 755 "$payload/bin/"* "$bin_dir/"

printf '\nInstalled OMP toolkit in %s\n' "$prefix"
# shellcheck disable=SC2016
printf 'Add to PATH: export PATH=%q:"$PATH"\n' "$bin_dir"
printf 'Register in Codex: codex mcp add omp -- %q --bin-dir %q --omp %q --node %q --profile-helper %q\n' \
    "$bin_dir/omp-codex-mcp" "$bin_dir" "$omp_bin" "$node_bin" "$runtime_dir/profile/summary.mjs"
