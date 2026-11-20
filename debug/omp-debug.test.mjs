import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { readFrame, responseLimit } from "./protocol.mjs";

const cli = fileURLToPath(new URL("./omp-debug.mjs", import.meta.url));
const available = process.platform === "darwin" &&
  spawnSync("xcrun", ["--find", "lldb-dap"], { timeout: 3000 }).status === 0;

function start(root, name) {
  const directory = join(root, name);
  const child = spawn(process.execPath, [cli, "--session", directory, "serve", "--root", root], {
    env: { ...process.env, OMP_DEBUG_BIN: join(root, "missing-omp-host") },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const ready = Promise.withResolvers();
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", chunk => {
    stdout += chunk;
    if (stdout.includes("OMP_DEBUG_READY ")) ready.resolve();
  });
  child.stderr.setEncoding("utf8").on("data", chunk => { stderr += chunk; });
  const closed = new Promise(resolve => {
    child.once("close", (code, signal) => {
      ready.reject(new Error(`Debug server closed before readiness: ${code ?? signal}\n${stdout}${stderr}`));
      resolve({ code, signal, stderr });
    });
  });
  return { directory, child, closed, ready: ready.promise };
}

function command(directory, ...args) {
  return spawnSync(process.execPath, [cli, "--session", directory, "--timeout", "8", ...args], {
    encoding: "utf8", timeout: 12000,
  });
}

function debug(directory, ...args) {
  const response = command(directory, ...args);
  assert.equal(response.status, 0, response.stdout + response.stderr);
  const result = JSON.parse(response.stdout);
  assert.equal(result.ok, true, response.stdout);
  return result.details;
}

function locals(directory) {
  const frame = debug(directory, "stack").stackFrames[0];
  const scope = debug(directory, "scopes", String(frame.id)).scopes.find(scope => scope.name === "Locals");
  return debug(directory, "variables", String(scope.variablesReference)).variables;
}

async function startContinue(directory) {
  const child = spawn(process.execPath, [cli, "--session", directory, "continue"], { stdio: "ignore" });
  const closed = new Promise(resolve => child.once("close", resolve));
  for (let attempt = 0; attempt < 25; attempt++) {
    const probe = command(directory, "status");
    if (probe.status === 2 && /Debug server is busy/.test(probe.stdout)) return { closed };
    await delay(20);
  }
  child.kill();
  assert.fail("the continue request never started");
}

test("pause waits until continue can resolve a missing DAP thread ID", {
  skip: process.platform === "win32", timeout: 15_000,
}, async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "od-")));
  const session = start(root, "delayed");
  t.after(async () => {
    session.child.kill("SIGTERM");
    await session.closed;
    rmSync(root, { recursive: true, force: true });
  });
  const adapter = join(root, "adapter");
  writeFileSync(adapter, readFileSync(new URL("./test-fixtures/delayed-threads.mjs", import.meta.url)), { mode: 0o700 });
  await session.ready;
  const launch = debug(session.directory, "launch", adapter, "--adapter", adapter);
  assert.equal(launch.snapshot.status, "stopped");
  assert.equal(launch.snapshot.threadId, undefined);

  const continued = await startContinue(session.directory);
  const threadsRequested = join(root, "threads-requested");
  for (let attempt = 0; attempt < 100 && !existsSync(threadsRequested); attempt++) await delay(20);
  assert.ok(existsSync(threadsRequested), "the adapter never received the threads request");

  const socket = createConnection(join(session.directory, "dap.sock"));
  t.after(() => socket.destroy());
  const connected = once(socket, "connect");
  await connected;
  const paused = readFrame(socket, responseLimit);
  socket.write('{"command":"pause","args":[],"timeout":8}\n');
  const respondedEarly = await Promise.race([paused.then(() => true), delay(200, false)]);
  writeFileSync(join(root, "release-threads"), "");
  assert.equal(respondedEarly, false, "pause reported success while continue was still finding a thread");

  const response = await paused;
  assert.equal(response.ok, true);
  assert.equal(response.details.snapshot.status, "stopped");
  assert.equal(response.details.snapshot.stopReason, "pause");
  assert.equal(await continued.closed, 0);
  debug(session.directory, "stop");
  assert.equal((await session.closed).code, 0);
});

test("native LLDB sessions work without the Oh My Pi host and support concurrent pause and stop", {
  skip: !available, timeout: 30_000,
}, async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "od-")));
  const sessions = [];
  t.after(async () => {
    for (const session of sessions) session.child.kill("SIGTERM");
    await Promise.all(sessions.map(session => session.closed));
    rmSync(root, { recursive: true, force: true });
  });

  const source = join(root, "target.c");
  writeFileSync(source, [
    "#include <unistd.h>",
    "static int score(int input) {",
    "    int total = input + 4;",
    "    total *= 2;",
    "    return total;",
    "}",
    "int main(int argc, char **argv) {",
    "    if (argc != 2 || argv[1][0] != '\\0') return 23;",
    "    if (score(3) != 14) return 24;",
    "    for (;;) sleep(1);",
    "}",
    "",
  ].join("\n"));
  const build = spawnSync("xcrun", ["clang", "-O0", "-g", source, "-o", join(root, "target")], { encoding: "utf8" });
  assert.equal(build.status, 0, build.stderr);

  const first = start(root, "first");
  sessions.push(first);
  await first.ready;
  const directory = first.directory;
  const failed = command(directory, "launch", "target", "--adapter", join(root, "missing-adapter"));
  assert.equal(failed.status, 2);
  assert.match(failed.stdout, /ENOENT/);
  assert.deepEqual(debug(directory, "status").sessions, []);
  assert.equal(debug(directory, "launch", "target", "--adapter", "lldb-dap", "--", "").snapshot.status, "stopped");
  assert.equal(debug(directory, "break", "target.c", "4").breakpoints[0].verified, true);
  assert.equal(debug(directory, "continue").state, "stopped");
  assert.deepEqual(debug(directory, "stack").stackFrames.slice(0, 2).map(frame => frame.name), ["score", "main"]);
  assert.equal(locals(directory).find(variable => variable.name === "total").value, "7");
  assert.equal(debug(directory, "step", "over").state, "stopped");
  assert.equal(locals(directory).find(variable => variable.name === "total").value, "14");

  const second = start(root, "second");
  sessions.push(second);
  await second.ready;
  assert.deepEqual(debug(second.directory, "status").sessions, []);
  const paused = await startContinue(directory);
  assert.equal(debug(directory, "pause").snapshot.status, "stopped");
  assert.equal(await paused.closed, 0);
  const running = await startContinue(directory);
  debug(directory, "stop");
  assert.equal(await running.closed, 2);
  assert.equal((await first.closed).code, 0);
  assert.equal(existsSync(directory), false);
  assert.deepEqual(debug(second.directory, "status").sessions, []);

  // An abandoned launcher closes the lease independently of the client connection.
  assert.equal(second.child.kill("SIGKILL"), true);
  assert.equal((await second.closed).signal, "SIGKILL");
  for (let attempt = 0; attempt < 50 && existsSync(second.directory); attempt++) await delay(40);
  assert.equal(existsSync(second.directory), false);
});
