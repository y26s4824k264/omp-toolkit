import { spawnSync } from 'node:child_process';
import { chmodSync, cpSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const upstream = join(root, 'target', 'upstream-source');
const [target] = process.argv.slice(2);
if (!target) throw new Error('Supply the Rust target triple');

const windows = process.platform === 'win32';
const suffix = windows ? '.exe' : '';
const targetDir = join(root, 'target');
const dist = join(targetDir, 'distribution');
const asset = `omp-toolkit-${process.platform}-${process.arch}.${windows ? 'zip' : 'tar.gz'}`;
const staging = mkdtempSync(join(targetDir, 'release-payload-'));
const bin = join(staging, 'bin');
const runtime = join(staging, 'runtime');
const licenses = join(runtime, 'licenses');
const shellTarget = join(targetDir, 'upstream-build', 'ci');

function copy(from, to) {
    mkdirSync(dirname(to), { recursive: true });
    cpSync(from, to, { recursive: true });
}

function run(command, args) {
    const result = spawnSync(command, args, { stdio: 'inherit' });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`${command} failed with status ${result.status}`);
}

try {
    for (const name of ['omp-refactor', 'omp-toolkit-mcp', 'omp-shell', 'obash']) {
        const from = {
            'omp-refactor': join(targetDir, 'release', 'omp-refactor'),
            'omp-toolkit-mcp': join(targetDir, 'release', 'omp-toolkit-mcp'),
            'omp-shell': join(shellTarget, 'examples', 'omp_shell_cli'),
            obash: join(shellTarget, 'obash'),
        }[name];
        const to = join(bin, name + suffix);
        copy(from + suffix, to);
        if (!windows) chmodSync(to, 0o755);
    }
    for (const component of ['debug', 'lsp', 'magic']) {
        for (const file of readdirSync(join(root, component), { withFileTypes: true })) {
            const runtimeFile = /\.(mjs|json|ts|md)$/.test(file.name) && !/\.test\.(mjs|ts)$/.test(file.name);
            if (file.isFile() && runtimeFile) {
                copy(join(root, component, file.name), join(runtime, component, file.name));
            }
        }
    }
    for (const file of [
        'lsp/servers/package.json', 'lsp/servers/package-lock.json',
        'profile', 'stats/omp-context-stats.mjs', 'THIRD_PARTY_NOTICES.md',
    ]) {
        copy(join(root, file), join(runtime, file));
    }

    for (const [file, filename] of [
        ['LICENSE', 'toolkit-LICENSE'],
        ['crates/ast/LICENSE', 'omp-ast-LICENSE'],
        ['crates/xfs/LICENSE', 'omp-xfs-LICENSE'],
        ['crates/vendor/grep-searcher/LICENSE-MIT', 'grep-searcher-LICENSE-MIT'],
        ['crates/vendor/grep-searcher/UNLICENSE', 'grep-searcher-UNLICENSE'],
        ['crates/vendor/tree-sitter-go/LICENSE', 'tree-sitter-go-LICENSE'],
        ['licenses/tree-sitter-ini-1.4.0-APACHE-2.0.txt', 'tree-sitter-ini-1.4.0-APACHE-2.0.txt'],
    ]) {
        copy(join(root, file), join(licenses, filename));
    }
    for (const [file, filename] of [
        ['LICENSE', 'oh-my-pi-LICENSE'],
        ['crates/shell/LICENSE', 'omp-shell-LICENSE'],
        ['crates/shell/NOTICE', 'omp-shell-NOTICE'],
    ]) {
        copy(join(upstream, file), join(licenses, filename));
    }

    run(process.execPath, [
        join(root, 'scripts', 'binary-notices.mjs'),
        '--toolkit-build', join(targetDir, 'toolkit-build.jsonl'),
        '--shell-build', join(targetDir, 'shell-build.jsonl'),
        '--upstream', upstream, '--target', target, '--out', licenses,
    ]);

    mkdirSync(dist, { recursive: true });
    const tar = windows ? resolve(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe') : 'tar';
    run(tar, [windows ? '-acf' : '-czf', join(dist, asset), '-C', staging, 'bin', 'runtime']);
    console.log(asset);
} finally {
    rmSync(staging, { recursive: true, force: true });
}
