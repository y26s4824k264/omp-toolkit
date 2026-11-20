import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const upstream = join(root, 'target', 'upstream-source');
const example = join(upstream, 'crates', 'shell', 'examples', 'omp_shell_cli');
mkdirSync(example, { recursive: true });
copyFileSync(join(root, 'shell', 'omp_shell_cli.rs'), join(example, 'main.rs'));
copyFileSync(join(root, 'src', 'log_preview.rs'), join(example, 'log_preview.rs'));

if (process.platform === 'darwin') {
    // Use the system open command instead of distributing AppKit SDK bindings.
    const path = join(upstream, 'crates', 'builtins', 'Cargo.toml');
    const source = readFileSync(path, 'utf8');
    const desktopOpen = /^[\t ]+"util\.open",\r?\n/gm;
    if ([...source.matchAll(desktopOpen)].length !== 1) {
        throw new Error('The pinned builtins feature list changed');
    }
    writeFileSync(path, source.replace(desktopOpen, ''));
}
