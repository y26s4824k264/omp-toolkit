import { createReadStream, realpathSync, statSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { Settings } from "@oh-my-pi/pi-coding-agent";
import { dapSessionManager } from "@oh-my-pi/pi-coding-agent/dap";
import { DebugTool } from "@oh-my-pi/pi-coding-agent/tools/debug";
import { integer, privateSession, readFrame, readyPrefix, removeSession, requestLimit, responseLimit } from "./protocol.mjs";

const plain = new Map([["continue", "continue"], ["pause", "pause"], ["status", "sessions"], ["stop", "terminate"]]);

function canonicalTarget(path, allowDirectory = false) {
  if (!path) throw new Error("Expected a nonempty path");
  const canonical = realpathSync(resolve(path));
  const info = statSync(canonical);
  if (!info.isFile() && !(allowDirectory && info.isDirectory())) throw new Error(`Invalid target path: ${path}`);
  return canonical;
}

function parameters({ command, args, adapter, timeout }) {
  if (typeof command !== "string" || !command || !Array.isArray(args) || !args.every(value => typeof value === "string")) {
    throw new Error("Expected a command and string arguments");
  }
  if (adapter !== undefined && (command !== "launch" || typeof adapter !== "string" || !adapter)) {
    throw new Error("--adapter applies only to launch");
  }
  const limit = integer(timeout, "--timeout", 5, 300);
  const action = plain.get(command);
  const [argument, second] = args;
  if (action && args.length === 0) return { action, timeout: limit };
  if (command === "launch" && args.length > 0) {
    return { action: "launch", program: canonicalTarget(argument, true), args: args.slice(1), adapter, timeout: limit };
  }
  if (command === "break" && args.length === 2) {
    return { action: "set_breakpoint", file: canonicalTarget(argument), line: integer(second, "LINE", 1, 2147483647), timeout: limit };
  }
  if (command === "step" && args.length <= 1 && ["over", "in", "out"].includes(argument ?? "over")) {
    return { action: `step_${argument ?? "over"}`, timeout: limit };
  }
  if (command === "stack" && args.length <= 1) {
    return { action: "stack_trace", levels: integer(argument ?? "20", "LEVELS", 1, 100), timeout: limit };
  }
  if (command === "scopes" && args.length === 1) {
    return { action: "scopes", frame_id: integer(argument, "FRAME_ID", 0, Number.MAX_SAFE_INTEGER), timeout: limit };
  }
  if (command === "variables" && args.length === 1) {
    return { action: "variables", variable_ref: integer(argument, "VARIABLE_REFERENCE", 1, Number.MAX_SAFE_INTEGER), timeout: limit };
  }
  throw new Error(`Invalid command or arguments: ${command}`);
}

function reply(socket, value) {
  const frame = JSON.stringify(value);
  socket.end(Buffer.byteLength(frame) <= responseLimit ? `${frame}\n` : '{"ok":false,"error":"Debug output exceeds the response limit"}\n');
}

function toolResponse(result) {
  const details = result.details;
  const output = result.content.filter(item => item.type === "text").map(item => item.text).join("\n");
  return { ok: details?.success === true && !details.timedOut, output, details };
}

export default async function run() {
  const socketPath = process.env.OMP_DEBUG_SOCKET;
  const location = { directory: dirname(socketPath), socket: socketPath };
  const original = privateSession(location.directory);
  const idleSeconds = integer(process.env.OMP_DEBUG_IDLE, "--idle", 1, 3600);
  const tool = new DebugTool({ cwd: process.cwd(), settings: Settings.isolated() });
  const sockets = new Set();
  const shutdown = Promise.withResolvers();
  let idleTimer;
  let active;
  let stopping = false;
  function stop(request) {
    if (stopping) return;
    stopping = true;
    clearTimeout(idleTimer);
    active?.controller.abort();
    shutdown.resolve(request);
  }
  function scheduleIdle() {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(stop, idleSeconds * 1000);
  }

  const server = createServer(socket => {
    sockets.add(socket);
    socket.on("error", () => {});
    socket.on("close", () => sockets.delete(socket));
    if (stopping || sockets.size > 8) {
      reply(socket, { ok: false, error: "Debug server is stopping or has too many clients" });
      return;
    }
    socket.setTimeout(5000, () => socket.destroy());
    void handle(socket);
  });

  async function handle(socket) {
    let current;
    try {
      const params = parameters(await readFrame(socket, requestLimit));
      if (socket.destroyed) return;
      if (stopping) throw new Error("Debug server is stopping");
      if (params.action === "terminate") {
        socket.setTimeout(0);
        stop({ socket, params });
        return;
      }
      if (active) throw new Error("Debug server is busy");
      clearTimeout(idleTimer);
      socket.setTimeout(0);
      const controller = new AbortController();
      const completed = Promise.withResolvers();
      current = { controller, completed };
      active = current;
      socket.once("close", () => controller.abort());
      const result = await tool.execute("omp-debug", params, controller.signal);
      if (!socket.destroyed) reply(socket, toolResponse(result));
    } catch (error) {
      if (!socket.destroyed) reply(socket, { ok: false, error: error.message });
    } finally {
      if (current) {
        active = undefined;
        current.completed.resolve();
        if (!stopping) scheduleIdle();
      }
    }
  }

  // The foreground launcher holds this pipe. Its exit closes it even on SIGKILL.
  const lease = createReadStream(null, { fd: 3 });
  const signal = () => stop();
  lease.once("end", signal);
  lease.once("error", signal);
  lease.resume();
  process.on("SIGINT", signal);
  process.on("SIGTERM", signal);
  process.on("SIGHUP", signal);
  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(socketPath, resolve);
    });
    server.on("error", signal);
    if (!stopping) {
      console.log(`${readyPrefix}${JSON.stringify({ session: location.directory, root: process.cwd() })}`);
      scheduleIdle();
    }
    const request = await shutdown.promise;
    const closed = new Promise(resolve => server.close(resolve));
    for (const socket of sockets) if (socket !== request?.socket) socket.destroy();
    await active?.completed.promise;
    if (request) {
      try {
        const result = await tool.execute("omp-debug-stop", request.params, AbortSignal.timeout(4000));
        if (!request.socket.destroyed) reply(request.socket, toolResponse(result));
      } catch (error) {
        if (!request.socket.destroyed) reply(request.socket, { ok: false, error: error.message });
      }
    } else {
      await dapSessionManager.terminate(AbortSignal.timeout(4000), 4000);
    }
    await closed;
    removeSession(location, original);
  } finally {
    lease.destroy();
    process.off("SIGINT", signal);
    process.off("SIGTERM", signal);
    process.off("SIGHUP", signal);
  }
}
