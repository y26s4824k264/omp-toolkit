import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("./omp-debug.mjs", import.meta.url));
const omp = process.env.OMP_DEBUG_BIN ?? join(homedir(), ".omp", "bin", "19.0.0", "omp");
const available = process.platform === "darwin" && existsSync(omp) &&
  spawnSync("xcrun", ["--find", "lldb-dap"], { timeout: 3000 }).status === 0;

function start(root, name) {
  const directory = join(root, name);
  const child = spawn(process.execPath, [cli, "--session", directory, "serve", "--root", root], {
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

test("native LLDB retains breakpoint state across CLI calls and stop interrupts an active continue", {
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
  const running = spawn(process.execPath, [cli, "--session", directory, "continue"], { stdio: "ignore" });
  const continued = new Promise(resolve => running.once("close", resolve));
  let busy;
  for (let attempt = 0; attempt < 25 && !busy; attempt++) {
    const probe = command(directory, "status");
    busy = probe.status === 2 && /Debug server is busy/.test(probe.stdout);
    if (!busy) await delay(20);
  }
  assert.ok(busy, "the continue request never started");
  debug(directory, "stop");
  assert.equal(await continued, 2);
  assert.equal((await first.closed).code, 0);
  assert.equal(existsSync(directory), false);
  assert.deepEqual(debug(second.directory, "status").sessions, []);

  // An abandoned launcher closes the lease independently of the client connection.
  assert.equal(second.child.kill("SIGKILL"), true);
  assert.equal((await second.closed).signal, "SIGKILL");
  for (let attempt = 0; attempt < 50 && existsSync(second.directory); attempt++) await delay(40);
  assert.equal(existsSync(second.directory), false);
});
