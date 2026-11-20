#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import { lstatSync, mkdirSync, realpathSync, statSync } from "node:fs";
import { createConnection } from "node:net";
import { basename, delimiter, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { integer, privateSession, readFrame, readyPrefix, removeSession, requestLimit, responseLimit } from "./protocol.mjs";

const help = `Usage: omp-debug --session ABSENT_DIR serve [--root PROJECT] [--idle SECONDS]
       omp-debug --session DIR [--timeout SECONDS] COMMAND [ARGS...]

Commands:
  launch PROGRAM [--adapter lldb-dap] [-- PROGRAM_ARG ...]
  break FILE LINE
  continue
  pause
  step [over|in|out]
  stack [LEVELS]
  scopes FRAME_ID
  variables VARIABLE_REFERENCE
  status
  stop

Wait for OMP_DEBUG_READY before sending requests. Responses are JSON.
The request timeout defaults to 30s. The server's idle timeout defaults to 900s.
Debug sessions require macOS or Linux with Unix domain sockets.
Use a target language's stdio debug adapter, such as lldb-dap.
`;

function sessionLocation(argument) {
  if (!argument || !isAbsolute(argument)) throw new Error("--session requires an absolute task directory");
  const path = resolve(argument);
  const directory = join(realpathSync(dirname(path)), basename(path));
  const socket = join(directory, "dap.sock");
  if (Buffer.byteLength(socket) > 103) throw new Error("Session path is too long; choose a shorter directory in /tmp");
  return { directory, socket };
}

function debuggerPath() {
  if (process.platform !== "darwin") return undefined;
  const result = spawnSync("xcrun", ["--find", "lldb-dap"], { encoding: "utf8", timeout: 3000 });
  const executable = result.stdout?.trim();
  return result.status === 0 && executable && isAbsolute(executable) ? dirname(executable) : undefined;
}

async function serve(location, values) {
  const root = realpathSync(values.root ?? process.cwd());
  if (!statSync(root).isDirectory()) throw new Error(`Expected a project directory: ${root}`);
  const idle = integer(values.idle ?? "900", "--idle", 1, 3600);
  let original;
  try {
    mkdirSync(location.directory, { mode: 0o700 });
    original = privateSession(location.directory);
    const bridge = fileURLToPath(new URL("./bridge.mjs", import.meta.url));
    const path = [debuggerPath(), dirname(process.execPath), process.env.PATH].filter(Boolean).join(delimiter);
    const child = spawn(process.execPath, [bridge], {
      cwd: root,
      env: {
        ...process.env, PATH: path,
        OMP_DEBUG_SOCKET: location.socket, OMP_DEBUG_IDLE: String(idle),
      },
      stdio: ["ignore", "pipe", "inherit", "pipe"],
    });
    let ready = false;
    let tail = "";
    const lease = child.stdio[3];
    lease.on("error", () => {});
    let killTimer;
    function signal() {
      lease.end();
      killTimer ??= setTimeout(() => child.kill("SIGKILL"), 10000);
    }
    let startupTimedOut = false;
    const startupTimer = setTimeout(() => {
      startupTimedOut = true;
      signal();
    }, 15000);
    child.stdout.setEncoding("utf8").on("data", chunk => {
      const text = tail + chunk;
      if (!ready && text.includes(readyPrefix)) {
        ready = true;
        clearTimeout(startupTimer);
      }
      tail = text.slice(-readyPrefix.length);
      process.stdout.write(chunk);
    });
    process.on("SIGINT", signal);
    process.on("SIGTERM", signal);
    process.on("SIGHUP", signal);
    try {
      const { code, reason } = await new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("close", (code, reason) => resolve({ code, reason }));
      });
      if (startupTimedOut) throw new Error("OMP debug server did not become ready within 15 seconds");
      if (!ready || code !== 0) throw new Error(`OMP debug server exited (${reason ?? code})`);
    } finally {
      clearTimeout(startupTimer);
      clearTimeout(killTimer);
      process.off("SIGINT", signal);
      process.off("SIGTERM", signal);
      process.off("SIGHUP", signal);
      lease.destroy();
    }
  } finally {
    if (original) removeSession(location, original);
  }
}

async function request(location, command, args, values) {
  privateSession(location.directory);
  if (!lstatSync(location.socket).isSocket()) throw new Error("Session endpoint is not a Unix socket");
  const timeout = integer(values.timeout ?? "30", "--timeout", 5, 300);
  const line = JSON.stringify({ command, args, adapter: values.adapter, timeout });
  if (Buffer.byteLength(line) > requestLimit) throw new Error(`Request exceeds ${requestLimit} bytes`);
  const socket = createConnection(location.socket);
  socket.setTimeout((timeout + 6) * 1000, () => socket.destroy(new Error("Debug request timed out")));
  try {
    const response = readFrame(socket, responseLimit);
    socket.write(`${line}\n`);
    const result = await response;
    console.log(JSON.stringify(result));
    if (!result.ok) process.exitCode = 2;
  } finally {
    socket.destroy();
  }
}

try {
  const { values, positionals } = parseArgs({ allowPositionals: true, options: {
    help: { type: "boolean", short: "h" }, session: { type: "string" }, root: { type: "string" },
    timeout: { type: "string" }, idle: { type: "string" }, adapter: { type: "string" },
  } });
  if (values.help || !positionals.length) {
    process.stdout.write(help);
  } else {
    if (process.platform === "win32") throw new Error("Debug sessions require macOS or Linux with Unix domain sockets");
    const [command, ...args] = positionals;
    const location = sessionLocation(values.session);
    if (command === "serve") {
      if (args.length || values.adapter || values.timeout) throw new Error("serve accepts --root and --idle");
      await serve(location, values);
    } else {
      if (values.root || values.idle) throw new Error("--root and --idle apply only to serve");
      await request(location, command, args, values);
    }
  }
} catch (error) {
  console.error(`omp-debug: ${error.message}`);
  process.exitCode = 2;
}
