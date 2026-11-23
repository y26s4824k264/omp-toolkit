#!/usr/bin/env bash
set -euo pipefail

repo=y26s4824k264/omp-toolkit
node_version=v24.22.3
version=latest
prefix=
client=auto
with_lsp_servers=true

usage() {
    cat <<'EOF'
Usage: bash install.sh [options]

  --prefix DIR        Install into DIR
  --version TAG       Install a release tag such as v0.2.0 (default: latest)
  --client NAME       Configure local clients: auto (default), none, codex, or claude
                      Auto detects Codex and Claude Code and configures MCP and global instructions
  --without-lsp-servers  Skip installing the default language servers
  --with-lsp-servers     Install the default language servers (already enabled)
  --help              Show this help
EOF
}

die() {
    printf 'install: %s\n' "$*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|--prefix|--client)
            [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || die "$1 requires a value"
            case "$1" in
                --version) version=$2 ;;
                --prefix) prefix=$2 ;;
                --client) client=$2 ;;
            esac
            shift 2
            ;;
        --with-lsp-servers) with_lsp_servers=true; shift ;;
        --without-lsp-servers) with_lsp_servers=false; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

case "$client" in
    auto|none|codex|claude) ;;
    *) die 'client must be auto, none, codex, or claude' ;;
esac

valid_tag() {
    [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]
}
[[ "$version" == latest ]] || valid_tag "$version" || die 'version must be latest or a release tag such as v0.2.0'

case "$(uname -s)" in
    Darwin) platform=darwin ;;
    Linux) platform=linux ;;
    *) die 'use install.ps1 on Windows' ;;
esac

command -v curl >/dev/null || die 'curl is required'
if command -v sha256sum >/dev/null; then
    checksum() { sha256sum "$1"; }
elif command -v shasum >/dev/null; then
    checksum() { shasum -a 256 "$1"; }
else
    die 'sha256sum or shasum is required'
fi

umask 077
stage_dir=$(mktemp -d "${TMPDIR:-/tmp}/omp-toolkit.XXXXXXXX")
trap 'rm -rf -- "$stage_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

download() {
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --retry 2 --connect-timeout 15 --max-time 300 --output "$2" "$1"
}

if [[ "$version" == latest ]]; then
    download "https://github.com/$repo/releases/latest/download/VERSION" "$stage_dir/VERSION"
    version=$(LC_ALL=C awk 'NR == 1 { sub(/\r$/, ""); tag = $0 } END { if (NR == 1) print tag }' "$stage_dir/VERSION")
    valid_tag "$version" || die 'GitHub returned an invalid release tag'
fi

verify() {
    local expected actual
    expected=$(awk -v asset="$2" '$2 == asset || $2 == "*" asset { print tolower($1) }' "$3")
    [[ "$expected" =~ ^[a-f0-9]{64}$ ]] || die "missing or invalid checksum for $2"
    actual=$(checksum "$1" | awk '{ print tolower($1) }')
    [[ "$actual" == "$expected" ]] || die "checksum mismatch for $2"
}

node_bin=$(command -v node || true)
npm_cli=
system_node_is_usable() {
    [[ "$node_bin" == /* ]] || return 1
    [[ "$("$node_bin" -p 'Number(process.versions.node.split(".")[0]) >= 24' 2>/dev/null)" == true ]] || return 1
    ! "$with_lsp_servers" || npm --version >/dev/null 2>&1
}

if ! system_node_is_usable; then
    case "$(uname -m)" in
        x86_64|amd64) arch=x64 ;;
        arm64|aarch64) arch=arm64 ;;
        *) die 'supported architectures are x64 and arm64' ;;
    esac
    command -v tar >/dev/null || die 'tar is required to install Node.js'
    node_dist="https://nodejs.org/dist/$node_version"
    node_name="node-$node_version-$platform-$arch"
    node_archive="$node_name.tar.gz"
    printf 'Downloading Node.js %s with npm for %s/%s\n' "$node_version" "$platform" "$arch"
    download "$node_dist/SHASUMS256.txt" "$stage_dir/node-checksums.txt"
    download "$node_dist/$node_archive" "$stage_dir/$node_archive"
    verify "$stage_dir/$node_archive" "$node_archive" "$stage_dir/node-checksums.txt"
    npm_dir="$node_name/lib/node_modules/npm"
    tar -xzf "$stage_dir/$node_archive" -C "$stage_dir" "$node_name/bin/node" "$npm_dir"
    node_bin="$stage_dir/$node_name/bin/node"
    npm_cli="$stage_dir/$npm_dir/bin/npm-cli.js"
    [[ -x "$node_bin" && -f "$npm_cli" ]] || die 'Node.js archive did not contain Node and npm'
fi

release="https://github.com/$repo/releases/download/$version"
download "$release/SHA256SUMS" "$stage_dir/SHA256SUMS"
download "$release/install-release.mjs" "$stage_dir/install-release.mjs"
verify "$stage_dir/install-release.mjs" install-release.mjs "$stage_dir/SHA256SUMS"

install_args=(--version "$version" --client "$client")
if [[ -n "$prefix" ]]; then install_args+=(--prefix "$prefix"); fi
if ! "$with_lsp_servers"; then install_args+=(--without-lsp-servers); fi
if [[ -n "$npm_cli" ]]; then install_args+=(--portable-node "$node_bin" --npm-cli "$npm_cli"); fi
"$node_bin" "$stage_dir/install-release.mjs" "${install_args[@]}"
