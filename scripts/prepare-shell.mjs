import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const upstream = resolve(process.argv[2] ?? join(root, 'target', 'upstream-source'));
const patch = join(root, 'shell', 'process-family.patch');
const apply = (...args) => spawnSync('git', ['apply', ...args, patch], { cwd: upstream, encoding: 'utf8' });
if (apply('--reverse', '--check').status !== 0) {
    const result = apply('--check');
    if (result.status !== 0) throw new Error(result.error?.message ?? result.stderr);
    const applied = apply();
    if (applied.status !== 0) throw new Error(applied.error?.message ?? applied.stderr);
}
const example = join(upstream, 'crates', 'shell', 'examples', 'omp_shell_cli');
mkdirSync(example, { recursive: true });
copyFileSync(join(root, 'shell', 'omp_shell_cli.rs'), join(example, 'main.rs'));
copyFileSync(join(root, 'src', 'log_preview.rs'), join(example, 'log_preview.rs'));

if (process.platform === 'darwin') {
    // Use the system open command instead of distributing AppKit SDK bindings.
    const path = join(upstream, 'crates', 'builtins', 'Cargo.toml');
    const source = readFileSync(path, 'utf8');
    const desktopOpen = /^[\t ]+"util\.open",\r?\n/gm;
    if ([...source.matchAll(desktopOpen)].length > 1) {
        throw new Error('The pinned builtins feature list changed');
    }
    writeFileSync(path, source.replace(desktopOpen, ''));
}
