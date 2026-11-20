import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../', import.meta.url));
const windows = process.platform === 'win32';
const suffix = windows ? '.exe' : '';
const asset = `omp-codex-toolkit-${process.platform}-${process.arch}.${windows ? 'zip' : 'tar.gz'}`;
const archive = join(repo, 'target', 'distribution', asset);
const version = /^version = "([^"]+)"/m.exec(readFileSync(join(repo, 'Cargo.toml'), 'utf8'))[1];
const temp = realpathSync(mkdtempSync(join(tmpdir(), 'omp-release-')));
const prefix = join(temp, 'Tool Kit');
const checksum = join(temp, 'SHA256SUMS');
const project = join(temp, 'Fixture Project');
const file = 'sample.ts';
const source = 'export const result = oldCall(7);\n';
const createdPatches = [];

function run(program, args, options = {}) {
    return execFileSync(program, args, { encoding: 'utf8', timeout: 150_000, ...options });
}

function launchInstalled(name, args) {
    const command = join(prefix, 'bin', name + (windows ? '.cmd' : ''));
    if (!windows) return run(command, args);
    return run('pwsh', ['-NoProfile', '-Command',
        '$arguments = @(ConvertFrom-Json $env:OMP_SMOKE_ARGUMENTS); & $env:OMP_SMOKE_LAUNCHER @arguments; exit $LASTEXITCODE',
    ], { env: { ...process.env, OMP_SMOKE_LAUNCHER: command, OMP_SMOKE_ARGUMENTS: JSON.stringify(args) } });
}

async function smokeMcp(config) {
    assert.equal(config.command, join(prefix, 'bin', 'omp-codex-mcp' + suffix));
    const server = spawn(config.command, config.args, {
        stdio: ['pipe', 'pipe', 'inherit'],
        signal: AbortSignal.timeout(90_000),
    });
    const closed = once(server, 'close');
    const responses = createInterface({ input: server.stdout })[Symbol.asyncIterator]();
    let id = 0;

    function send(message) {
        server.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
    }

    async function request(method, params = {}) {
        const requestId = ++id;
        send({ id: requestId, method, params });
        for (;;) {
            const message = await responses.next();
            assert.equal(message.done, false, `MCP exited during ${method}`);
            const reply = JSON.parse(message.value);
            if (reply.id !== requestId) continue;
            assert.equal(reply.error, undefined, JSON.stringify(reply.error));
            return reply.result;
        }
    }

    async function call(name, args) {
        const reply = await request('tools/call', { name, arguments: args });
        assert.notEqual(reply.isError, true, `${name}: ${reply.content[0].text}`);
        console.log(`MCP ${name}: OK`);
        return reply.content[0].text;
    }

    function verifyPatch(text) {
        const patch = /^Full patch: (.+)$/m.exec(text)?.[1];
        assert.ok(patch, text);
        createdPatches.push(patch);
        run('git', ['-C', project, 'apply', '--check', '--', patch]);
    }

    try {
        const init = await request('initialize', {
            protocolVersion: '2024-11-05',
            capabilities: {},
            clientInfo: { name: 'release-smoke-test', version: '1' },
        });
        assert.equal(init.serverInfo.version, version);
        send({ method: 'notifications/initialized' });
        const tools = (await request('tools/list')).tools.map(tool => tool.name).sort();
        assert.deepEqual(tools, [
            'ast_find', 'ast_preview', 'hashline_preview', 'hashline_read', 'log_minimize',
            'profile_summary', 'read_summary', 'service_ready', 'shell_parse', 'token_count',
        ]);

        const root = project;
        const read = await call('hashline_read', { root, file, start: 1, end: 1 });
        assert.match(read, /oldCall\(7\)/);
        const header = /^\[sample\.ts#[A-Fa-f0-9]+\]$/m.exec(read)?.[0];
        assert.ok(header, read);
        verifyPatch(await call('hashline_preview', {
            root, files: [file], patch: `${header}\nPUT 1.=1:\n+export const result = newCall(7);\n`,
        }));
        const search = { root, files: [file], pattern: 'oldCall($VALUE)', language: 'typescript' };
        assert.match(await call('ast_find', search), /oldCall/);
        verifyPatch(await call('ast_preview', { ...search, replacement: 'newCall($VALUE)' }));
        assert.match(await call('read_summary', { root, file }), /result/);
        assert.match(await call('shell_parse', { script: 'printf hello' }), /printf/);
        assert.match(await call('log_minimize', {
            root, file: 'server.log', exit_code: 0, command: 'printf READY',
        }), /READY/);
        assert.match(await call('token_count', { root, file }), /o200k/);
        const readiness = await call('service_ready', {
            root, log: 'server.log', log_pattern: 'READY', log_offset: 0, pid: process.pid,
        });
        assert.equal(JSON.parse(readiness).status, 'ready');
        assert.match(await call('profile_summary', { root, file: 'fixture.cpuprofile' }), /work/);
        assert.equal(readFileSync(join(project, file), 'utf8'), source);
        server.stdin.end();
        assert.deepEqual(await closed, [0, null]);
    } finally {
        server.stdin.end();
        if (server.exitCode === null) server.kill();
    }
}

try {
    const sha256 = createHash('sha256').update(readFileSync(archive)).digest('hex');
    writeFileSync(checksum, `${sha256}  ${asset}\n`);
    const args = ['scripts/install-release.mjs', '--version', `v${version}`, '--client', 'none',
        '--archive', archive, '--checksum', checksum, '--prefix', prefix];
    writeFileSync(checksum, `${'0'.repeat(64)}  ${asset}\n`);
    const corrupt = spawnSync(process.execPath, args, { cwd: repo, encoding: 'utf8', timeout: 30_000 });
    assert.equal(corrupt.error, undefined);
    assert.notEqual(corrupt.status, 0);
    assert.match((corrupt.stderr || '') + (corrupt.stdout || ''), /checksum|sha.?256/i);
    writeFileSync(checksum, `${sha256}  ${asset}\n`);
    if (process.env.OMP_TEST_HOST) args.push('--omp-bin', process.env.OMP_TEST_HOST);
    run(process.execPath, args, { cwd: repo, stdio: 'inherit' });

    run('git', ['init', '--quiet', project]);
    writeFileSync(join(project, file), source);
    writeFileSync(join(project, 'server.log'), 'READY on loopback\n');
    writeFileSync(join(project, 'fixture.cpuprofile'), JSON.stringify({
        nodes: [{ id: 1, callFrame: { functionName: 'work', url: 'file:///sample.ts', lineNumber: 0 } }],
        startTime: 0, endTime: 1000, samples: [1], timeDeltas: [1000],
    }));

    const config = JSON.parse(readFileSync(join(prefix, 'mcp-config.json'), 'utf8')).mcpServers.omp;
    const vscode = JSON.parse(readFileSync(join(prefix, 'vscode-mcp.json'), 'utf8')).servers.omp;
    assert.deepEqual(vscode, { ...config, type: 'stdio' });
    await smokeMcp(config);

    const catalog = JSON.parse(launchInstalled('omp-lsp', ['--json', 'catalog']));
    assert.equal(catalog.servers.length, 55);
    launchInstalled('omp-debug', ['--help']);
    launchInstalled('codex-context-stats', ['--help']);

    const shell = join(prefix, 'bin', 'omp-shell' + suffix);
    const shellResult = spawnSync(shell, [
        'run-log', '--log', join(temp, 'exit.log'), '--', process.execPath, '-e',
        'console.log("release fixture"); process.exit(7)',
    ], { encoding: 'utf8', timeout: 30_000 });
    assert.equal(shellResult.error, undefined);
    assert.equal(shellResult.status, 7, shellResult.stderr);
    assert.match(readFileSync(join(temp, 'exit.log'), 'utf8'), /release fixture/);
    console.log('Release package and CLI smoke checks: OK');
} finally {
    for (const patch of createdPatches) rmSync(patch, { force: true });
    rmSync(temp, { recursive: true, force: true });
}
