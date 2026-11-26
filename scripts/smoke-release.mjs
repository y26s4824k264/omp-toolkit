import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../', import.meta.url));
const windows = process.platform === 'win32';
const suffix = windows ? '.exe' : '';
const asset = `omp-toolkit-${process.platform}-${process.arch}.${windows ? 'zip' : 'tar.gz'}`;
const archive = join(repo, 'target', 'distribution', asset);
const version = /^version = "([^"]+)"/m.exec(readFileSync(join(repo, 'Cargo.toml'), 'utf8'))[1];
const temp = realpathSync.native(mkdtempSync(join(tmpdir(), 'omp-release-')));
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
    assert.equal(realpathSync.native(config.command),
        realpathSync.native(join(prefix, 'bin', 'omp-toolkit-mcp' + suffix)));
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
        assert.equal(init.serverInfo.name, 'omp-toolkit-mcp');
        send({ method: 'notifications/initialized' });
        const tools = (await request('tools/list')).tools.map(tool => tool.name).sort();
        assert.deepEqual(tools, [
            'ast_find', 'ast_preview', 'hashline_preview', 'hashline_read', 'image_read',
            'log_minimize', 'office_read', 'profile_summary', 'read_summary', 'service_ready',
            'shell_parse', 'token_count',
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
        const counts = JSON.parse(await call('token_count', { root, file }));
        assert.equal(counts.encodings.length, 6);
        assert.ok(counts.encodings.some(item => item.name === 'o200k_base'));
        const readiness = await call('service_ready', {
            root, log: 'server.log', log_pattern: 'READY', log_offset: 0, pid: process.pid,
        });
        assert.equal(JSON.parse(readiness).status, 'ready');
        assert.match(await call('profile_summary', { root, file: 'fixture.cpuprofile' }), /work/);
        const image = await request('tools/call', {
            name: 'image_read', arguments: { root, file: 'fixture.png' },
        });
        assert.equal(image.isError, false);
        assert.equal(image.content.length, 1);
        assert.equal(JSON.parse(image.content[0].text).width, 1);
        const preview = await request('tools/call', {
            name: 'image_read', arguments: { root, file: 'fixture.png', max_edge: 32 },
        });
        assert.equal(preview.isError, false);
        assert.equal(preview.content[1].type, 'image');
        assert.equal(preview.content[1].mimeType, 'image/png');
        assert.equal(readFileSync(join(project, file), 'utf8'), source);
        server.stdin.end();
        assert.deepEqual(await closed, [0, null]);
    } finally {
        server.stdin.end();
        if (server.exitCode === null) server.kill();
    }
}

async function smokeRunLogCancellation(shell, signal, code) {
    const log = join(temp, `${signal}.log`);
    const pidFile = join(temp, `${signal}.pid`);
    const worker = `
      process.on('SIGINT', () => {});
      process.on('SIGTERM', () => {});
      require('node:fs').writeFileSync(process.argv[1], String(process.pid));
      console.log('detached worker ready');
      setInterval(() => {}, 1000);
    `;
    const foreground = `
      require('node:child_process').spawn(process.execPath,
        ['-e', process.argv[1], process.argv[2]], { detached: true, stdio: 'inherit' });
      setInterval(() => {}, 1000);
    `;
    const invocation = spawn(shell, ['run-log', '--log', log, '--', process.execPath, '-e', foreground,
        worker, pidFile], { stdio: 'ignore' });
    const closed = once(invocation, 'close');
    let workerPid;
    try {
        const deadline = Date.now() + 10_000;
        while (Date.now() < deadline) {
            if (invocation.exitCode !== null) break;
            try {
                workerPid = Number(readFileSync(pidFile, 'utf8'));
                if (workerPid > 0 && readFileSync(log, 'utf8').includes('detached worker ready')) break;
            } catch (error) {
                if (error.code !== 'ENOENT') throw error;
            }
            await delay(20);
        }
        assert.ok(workerPid > 0);
        assert.match(readFileSync(log, 'utf8'), /detached worker ready/);
        assert.ok(invocation.kill(signal));
        const result = await Promise.race([closed, delay(10_000, null, { ref: false })]);
        assert.deepEqual(result, [code, null]);
        const tree = spawnSync(shell, ['process-tree', String(workerPid), '--depth', '0', '--json'],
            { encoding: 'utf8', timeout: 5_000 });
        assert.equal(tree.error, undefined);
        if (tree.status === 0) {
            assert.equal(JSON.parse(tree.stdout).processes[0].status, 'exited');
        } else {
            assert.equal(tree.status, 1, tree.stderr);
            assert.match(tree.stderr, /cannot inspect PID/i);
        }
        workerPid = undefined;
        assert.match(readFileSync(log, 'utf8'), /detached worker ready/);
    } finally {
        if (invocation.exitCode === null) invocation.kill('SIGKILL');
        if (workerPid > 0) {
            try { process.kill(workerPid, 'SIGKILL'); } catch (error) {
                if (error.code !== 'ESRCH') throw error;
            }
        }
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
    run(process.execPath, args, { cwd: repo, stdio: 'inherit', timeout: 360_000 });
    assert.match(run(join(prefix, 'bin', 'omp-toolkit-mcp' + suffix), ['--version']), /omp-toolkit-mcp/);

    run('git', ['init', '--quiet', project]);
    writeFileSync(join(project, file), source);
    writeFileSync(join(project, 'server.log'), 'READY on loopback\n');
    writeFileSync(join(project, 'fixture.png'), Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC',
        'base64',
    ));
    writeFileSync(join(project, 'fixture.cpuprofile'), JSON.stringify({
        nodes: [{ id: 1, callFrame: { functionName: 'work', url: 'file:///sample.ts', lineNumber: 0 } }],
        startTime: 0, endTime: 1000, samples: [1], timeDeltas: [1000],
    }));

    const config = JSON.parse(readFileSync(join(prefix, 'mcp-config.json'), 'utf8')).mcpServers.omp;
    const vscode = JSON.parse(readFileSync(join(prefix, 'vscode-mcp.json'), 'utf8')).servers.omp;
    assert.deepEqual(vscode, { ...config, type: 'stdio' });
    const agentInstructions = readFileSync(join(prefix, 'AGENT-INSTRUCTIONS.md'), 'utf8');
    assert.match(agentInstructions, /omp-magic prepare/);
    assert.match(agentInstructions, /omp-refactor sloppy/);
    assert.match(agentInstructions, /omp-refactor learn skill update/);
    assert.match(agentInstructions, /omp-context-stats --turn 1/);
    await smokeMcp(config);

    const refactor = join(prefix, 'bin', 'omp-refactor' + suffix);
    const ranked = JSON.parse(run(refactor, [
        '--root', project, 'ranked-find', 'oldCall result', '--limit', '2',
    ]));
    assert.equal(ranked.results[0].path, file);
    const worktree = join(temp, 'Linked Worktree');
    run('git', ['-C', project, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.com',
        'commit', '--quiet', '--allow-empty', '-m', 'worktree fixture']);
    run('git', ['-C', project, 'worktree', 'add', '--quiet', '--detach', worktree, 'HEAD']);
    writeFileSync(join(project, '.git', 'info', 'exclude'), 'ignored.txt\n');
    writeFileSync(join(worktree, 'ignored.txt'), 'worktreemarker\n');
    writeFileSync(join(worktree, 'visible.txt'), 'worktreemarker\n');
    assert.equal(run('git', ['-C', worktree, 'check-ignore', 'ignored.txt']).trim(), 'ignored.txt');
    const worktreeResults = JSON.parse(run(refactor, ['--root', worktree, 'ranked-find', 'worktreemarker']));
    assert.deepEqual(worktreeResults.results.map(entry => entry.path), ['visible.txt']);

    writeFileSync(join(project, 'change.edit'), `*** Edit File: ${file}\n*** Find\noldCall(7)\n*** Replace\nnewCall(7)\n`);
    const anchoredPatch = run(refactor, ['--root', project, 'sloppy', '--patch', join(project, 'change.edit'), file]);
    assert.match(anchoredPatch, /newCall\(7\)/);
    run('git', ['-C', project, 'apply', '--check', '-'], { input: anchoredPatch });

    const plan = join(project, 'magic-plan.json');
    const intent = join(project, 'magic-intent.md');
    const replies = join(project, 'magic-replies.json');
    writeFileSync(intent, `# ${file}\n- Replace oldCall with newCall.\n`);
    writeFileSync(plan, launchInstalled('omp-magic', ['prepare', '--root', project, '--instructions', intent]));
    writeFileSync(replies, JSON.stringify({ replies: [{ path: file, operations: '["sub",1,"oldCall","newCall"]' }] }));
    const magicPatch = launchInstalled('omp-magic', ['preview', '--root', project, '--plan', plan, '--replies', replies]);
    assert.match(magicPatch, /newCall\(7\)/);
    run('git', ['-C', project, 'apply', '--check', '-'], { input: magicPatch });
    assert.equal(readFileSync(join(project, file), 'utf8'), source);

    const learn = (...args) => run(refactor, ['--root', project, 'learn', ...args]);
    assert.deepEqual(JSON.parse(learn('list')), []);
    const lesson = 'Review the complete patch before applying it.';
    assert.equal(JSON.parse(learn('save', lesson)).created, true);
    assert.equal(JSON.parse(learn('save', lesson)).created, false);
    assert.deepEqual(JSON.parse(learn('list')), [{ lesson }]);
    writeFileSync(join(project, 'skill-body.md'), 'Review and validate the changes.\n');
    const skill = ['patch-review', '--description', 'Review a patch', '--body-file', 'skill-body.md'];
    const preview = learn('skill', 'preview', ...skill);
    learn('skill', 'create', ...skill);
    assert.equal(readFileSync(join(project, '.omp-toolkit', 'learning', 'skills', skill[0], 'SKILL.md'), 'utf8'), preview);

    const goal = (...args) => JSON.parse(run(refactor, ['--root', project, 'goal', ...args]));
    goal('init', 'delivery', '--objective', 'Roll out the change', '--criterion', 'Required tests pass', '--criterion', 'Release is downloadable');
    goal('focus', 'delivery', 'Run the focused tests');
    assert.equal(goal('record', 'delivery', '1', 'All required test suites passed').ready_for_review, false);
    const restored = goal('show', 'delivery');
    assert.equal(restored.objective, 'Roll out the change');
    assert.equal(restored.current_milestone, 'Run the focused tests');
    assert.equal(goal('record', 'delivery', '2', 'Downloaded the release artifact').ready_for_review, true);
    assert.equal(goal('clear-evidence', 'delivery', '1').ready_for_review, false);

    const catalog = JSON.parse(launchInstalled('omp-lsp', ['--json', 'catalog']));
    assert.equal(catalog.servers.length, 55);
    writeFileSync(join(project, 'invalid.json'), '{"missing":}\n');
    assert.throws(() => launchInstalled('omp-lsp', [
        '--root', project, '--json', 'check', 'invalid.json',
    ]), error => error.status === 1 && JSON.parse(error.stdout).results[0].status === 'errors');
    launchInstalled('omp-debug', ['--help']);
    const contextHelp = launchInstalled('omp-context-stats', ['--help']);
    assert.match(contextHelp, /Usage: omp-context-stats/);
    assert.match(contextHelp, /Codex rollout JSONL/);

    const shell = join(prefix, 'bin', 'omp-shell' + suffix);
    const shellResult = spawnSync(shell, [
        'run-log', '--log', join(temp, 'exit.log'), '--', process.execPath, '-e',
        'console.log("release fixture"); process.exit(7)',
    ], { encoding: 'utf8', timeout: 30_000 });
    assert.equal(shellResult.error, undefined);
    assert.equal(shellResult.status, 7, shellResult.stderr);
    assert.match(readFileSync(join(temp, 'exit.log'), 'utf8'), /release fixture/);
    if (!windows) {
        await smokeRunLogCancellation(shell, 'SIGTERM', 143);
        await smokeRunLogCancellation(shell, 'SIGINT', 130);
    }
    const tree = JSON.parse(run(shell, ['process-tree', String(process.pid), '--depth', '0', '--json']));
    assert.equal(tree.processes[0].pid, process.pid);
    assert.equal(tree.processes[0].status, 'running');
    console.log('Release package and CLI smoke checks: OK');
} finally {
    for (const patch of createdPatches) rmSync(patch, { force: true });
    rmSync(temp, { recursive: true, force: true });
}
