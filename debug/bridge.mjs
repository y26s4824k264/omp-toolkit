import { spawn } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { realpathSync, statSync } from "node:fs";
import { createServer, Socket } from "node:net";
import { basename, dirname, extname, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { integer, privateSession, readFrame, readyPrefix, removeSession, requestLimit, responseLimit } from "./protocol.mjs";

const plain = new Map([["continue", "continue"], ["pause", "pause"], ["status", "sessions"], ["stop", "terminate"]]);
const resumeCommands = new Map([["continue", "continue"], ["step_over", "next"], ["step_in", "stepIn"], ["step_out", "stepOut"]]);
const dapHeaderLimit = 8 * 1024;
const dapBodyLimit = 4 * 1024 * 1024;

function openAdapter(command, args, onEvent, onClose) {
  const child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"] });
  const pending = new Map();
  let seq = 0;
  let buffered = Buffer.alloc(0);
  let messageLength;
  let failure;
  let stderr = "";

  function fail(error) {
    if (failure) return;
    failure = error;
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
    onClose();
  }

  const closed = new Promise(resolve => child.once("close", (code, signal) => {
    const suffix = stderr.trim() ? `: ${stderr.trim()}` : "";
    fail(new Error(`Debug adapter exited (${signal ?? code})${suffix}`));
    resolve();
  }));
  child.once("error", fail);
  child.stdin.on("error", fail);
  child.stderr.setEncoding("utf8").on("data", chunk => { stderr = (stderr + chunk).slice(-2048); });

  function send(message) {
    const body = Buffer.from(JSON.stringify({ ...message, seq: ++seq }));
    child.stdin.write(Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]));
    return seq;
  }

  function receive(message) {
    if (message.type === "event") {
      onEvent(message);
    } else if (message.type === "response") {
      const request = pending.get(message.request_seq);
      if (!request) return;
      pending.delete(message.request_seq);
      if (message.success) request.resolve(message.body ?? {});
      else request.reject(new Error(`${request.command}: ${message.body?.error?.format ?? message.message ?? "adapter request failed"}`));
    } else if (message.type === "request") {
      send({ type: "response", request_seq: message.seq, command: message.command, success: false,
        message: "This client supports only adapters that can run the target without a reverse request" });
    }
  }

  child.stdout.on("data", chunk => {
    if (failure) return;
    try {
      buffered = Buffer.concat([buffered, chunk]);
      while (buffered.length) {
        if (messageLength === undefined) {
          const separator = buffered.indexOf("\r\n\r\n");
          if (separator < 0 && buffered.length <= dapHeaderLimit) return;
          if (separator < 0 || separator > dapHeaderLimit) throw new Error("Invalid DAP header");
          const header = buffered.subarray(0, separator).toString("ascii");
          const size = /^Content-Length:\s*(\d+)\s*$/im.exec(header);
          messageLength = Number(size?.[1]);
          if (!Number.isSafeInteger(messageLength) || messageLength < 1 || messageLength > dapBodyLimit) {
            throw new Error("Invalid DAP content length");
          }
          buffered = buffered.subarray(separator + 4);
        }
        if (buffered.length < messageLength) return;
        const message = JSON.parse(buffered.subarray(0, messageLength).toString("utf8"));
        buffered = buffered.subarray(messageLength);
        messageLength = undefined;
        receive(message);
      }
    } catch (error) {
      fail(error);
      child.kill();
    }
  });

  return {
    get failure() { return failure; },
    async request(command, args, signal) {
      signal.throwIfAborted();
      if (failure) throw failure;
      const result = Promise.withResolvers();
      const id = send({ type: "request", command, arguments: args });
      pending.set(id, { ...result, command });
      const abort = () => result.reject(signal.reason);
      signal.addEventListener("abort", abort, { once: true });
      try {
        return await result.promise;
      } finally {
        pending.delete(id);
        signal.removeEventListener("abort", abort);
      }
    },
    async close() {
      child.stdin.end();
      const finished = () => child.exitCode !== null || child.signalCode !== null || child.pid === undefined;
      await Promise.race([closed, delay(250)]);
      if (!finished()) {
        child.kill();
        await Promise.race([closed, delay(500)]);
      }
      if (!finished()) child.kill("SIGKILL");
      child.stdout.destroy();
      child.stderr.destroy();
      await closed;
    },
  };
}

function adapterFor(requested, program) {
  const name = requested ?? (extname(program) === ".py" ? "debugpy" : "lldb-dap");
  if (name === "debugpy") {
    return { name, command: "python3", args: ["-m", "debugpy.adapter"], launch: { console: "internalConsole", justMyCode: false } };
  }
  switch (basename(name)) {
    case "gdb":
      return { name, command: name, args: ["-i", "dap"], launch: { stopAtBeginningOfMainSubprogram: true } };
    case "lldb-dap":
      return { name, command: name, args: [], launch: { disableASLR: false } };
    default:
      return { name, command: name, args: [], launch: {} };
  }
}

class DebugSession {
  #initialized = false;
  #state = "launching";
  #preparingResume = false;
  #revision = 0;
  #stopped = {};
  #exitCode;
  #updates = new EventEmitter();
  #breakpoints = new Map();

  constructor(program, adapterName) {
    this.program = program;
    this.cwd = process.cwd();
    this.launchedAt = new Date().toISOString();
    this.adapter = adapterFor(adapterName, program);
    this.connection = openAdapter(this.adapter.command, this.adapter.args,
      event => this.#onEvent(event), () => this.#onEvent({ event: "terminated" }));
  }

  #onEvent({ event, body = {} }) {
    switch (event) {
      case "initialized":
        this.#initialized = true;
        break;
      case "stopped":
        this.#state = "stopped";
        this.#stopped = body;
        this.#revision++;
        break;
      case "continued":
        this.#state = "running";
        this.#revision++;
        break;
      case "exited":
        this.#exitCode = body.exitCode;
        this.#state = "terminated";
        this.#revision++;
        break;
      case "terminated":
        this.#state = "terminated";
        this.#revision++;
        break;
      default:
        return;
    }
    this.#updates.emit("change");
  }

  async #waitFor(predicate, signal) {
    while (!predicate()) {
      if (this.connection.failure) throw this.connection.failure;
      await once(this.#updates, "change", { signal });
    }
  }

  async #waitForStop(signal) {
    await this.#waitFor(() => this.#state === "stopped" || this.#state === "terminated", signal);
  }

  async #threadId(signal) {
    const thread = this.#stopped.threadId ?? (await this.connection.request("threads", {}, signal)).threads?.[0]?.id;
    if (!Number.isSafeInteger(thread)) throw new Error("The debug adapter did not report a thread");
    return thread;
  }

  snapshot() {
    return { id: "1", adapter: this.adapter.name, cwd: this.cwd, program: this.program, launchedAt: this.launchedAt,
      status: this.#state, threadId: this.#stopped.threadId, stopReason: this.#state === "stopped" ? this.#stopped.reason : undefined,
      exitCode: this.#exitCode, breakpointFiles: this.#breakpoints.size,
      breakpointCount: [...this.#breakpoints.values()].reduce((sum, lines) => sum + lines.length, 0) };
  }

  async launch(args, signal) {
    const capabilities = await this.connection.request("initialize", {
      clientID: "omp-debug", adapterID: basename(this.adapter.name), pathFormat: "path", linesStartAt1: true, columnsStartAt1: true,
    }, signal);
    const launch = this.connection.request("launch", {
      program: this.program, cwd: this.cwd, args, stopOnEntry: true, ...this.adapter.launch,
    }, signal);
    if (capabilities.supportsConfigurationDoneRequest) {
      const configure = async () => {
        await this.#waitFor(() => this.#initialized, signal);
        if (capabilities.supportsExceptionFilterOptions || capabilities.exceptionBreakpointFilters?.length) {
          await this.connection.request("setExceptionBreakpoints", { filters: [] }, signal);
        }
        await this.connection.request("configurationDone", {}, signal);
      };
      await Promise.all([launch, configure()]);
    } else {
      await launch;
    }
    if (this.#state === "launching") this.#state = "running";
    const stopWindow = AbortSignal.timeout(3000);
    try {
      await this.#waitForStop(AbortSignal.any([signal, stopWindow]));
    } catch (error) {
      if (signal.aborted || !stopWindow.aborted) throw error;
    }
  }

  async #resume(command, signal) {
    if (this.#state !== "stopped") throw new Error("The debuggee needs to be stopped before resuming");
    let request;
    let revision;
    this.#preparingResume = true;
    try {
      const threadId = await this.#threadId(signal);
      signal.throwIfAborted();
      if (this.#state !== "stopped") throw new Error("The debuggee stopped being eligible to resume");
      this.#stopped.threadId ??= threadId;
      revision = this.#revision;
      this.#state = "running";
      request = this.connection.request(command, { threadId }, signal);
    } finally {
      this.#preparingResume = false;
      this.#updates.emit("change");
    }
    try {
      await request;
    } catch (error) {
      if (!signal.aborted && this.#revision === revision) this.#state = "stopped";
      throw error;
    }
    await this.#waitForStop(signal);
  }

  async execute(params, signal) {
    const resume = resumeCommands.get(params.action);
    if (resume) {
      try {
        await this.#resume(resume, signal);
        return answer({ snapshot: this.snapshot(), state: this.#state, timedOut: false }, `Program ${this.#state}.`);
      } catch (error) {
        if (signal.reason?.name === "TimeoutError" && this.#state === "running") {
          return answer({ snapshot: this.snapshot(), state: this.#state, timedOut: true }, "Program is still running. Use pause to inspect it.", false);
        }
        throw error;
      }
    }
    switch (params.action) {
      case "pause": {
        // A resume can still be resolving its thread while the previous stop is visible.
        await this.#waitFor(() => !this.#preparingResume, signal);
        if (this.#state === "running") {
          const threadId = await this.#threadId(signal);
          if (this.#state === "running") await this.connection.request("pause", { threadId }, signal);
          await this.#waitForStop(signal);
        }
        return answer({ snapshot: this.snapshot() }, `Program ${this.#state}.`);
      }
      case "set_breakpoint": {
        const breakpoints = [...new Set([...(this.#breakpoints.get(params.file) ?? []), params.line])].sort((a, b) => a - b);
        const body = await this.connection.request("setBreakpoints", {
          source: { path: params.file, name: basename(params.file) }, breakpoints: breakpoints.map(line => ({ line })),
        }, signal);
        this.#breakpoints.set(params.file, breakpoints);
        return answer({ snapshot: this.snapshot(), breakpoints: body.breakpoints }, `Breakpoints for ${params.file}.`);
      }
      case "stack_trace":
      case "scopes":
      case "variables": {
        if (this.#state !== "stopped") throw new Error("Pause the debuggee before reading its stack or variables");
        const request = async () => {
          if (params.action === "scopes") return this.connection.request("scopes", { frameId: params.frame_id }, signal);
          if (params.action === "variables") return this.connection.request("variables", { variablesReference: params.variable_ref }, signal);
          const threadId = await this.#threadId(signal);
          return this.connection.request("stackTrace", { threadId, startFrame: 0, levels: params.levels }, signal);
        };
        return answer({ snapshot: this.snapshot(), ...await request() }, "Debug data ready.");
      }
      default:
        throw new Error(`Unsupported debug action: ${params.action}`);
    }
  }

  async stop() {
    try {
      if (!this.connection.failure) {
        await this.connection.request("disconnect", { terminateDebuggee: true }, AbortSignal.timeout(1500));
      }
    } catch (error) {
      if (!this.connection.failure && this.#state !== "terminated") throw error;
    } finally {
      await this.connection.close();
    }
    return answer({ snapshot: this.snapshot() }, "Debug session terminated.");
  }
}

function answer(details, output, ok = true) {
  return { ok, output, details: { success: true, ...details } };
}

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

async function run() {
  const socketPath = process.env.OMP_DEBUG_SOCKET;
  const location = { directory: dirname(socketPath), socket: socketPath };
  const original = privateSession(location.directory);
  const idleSeconds = integer(process.env.OMP_DEBUG_IDLE, "--idle", 1, 3600);
  const sockets = new Set();
  const active = new Set();
  const shutdown = Promise.withResolvers();
  let session;
  let idleTimer;
  let stopping = false;
  function stop(request) {
    if (stopping) return;
    stopping = true;
    clearTimeout(idleTimer);
    for (const job of active) job.controller.abort();
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
        stop({ socket });
        return;
      }
      const canPause = params.action === "pause" && active.size === 1 && resumeCommands.has(active.values().next().value.action);
      if (active.size && !canPause) throw new Error("Debug server is busy");
      clearTimeout(idleTimer);
      socket.setTimeout(0);
      const controller = new AbortController();
      const completed = Promise.withResolvers();
      current = { controller, completed, action: params.action };
      active.add(current);
      socket.once("close", () => controller.abort());
      const signal = AbortSignal.any([controller.signal, AbortSignal.timeout(params.timeout * 1000)]);
      const result = await execute(params, signal);
      if (!socket.destroyed) reply(socket, result);
    } catch (error) {
      if (!socket.destroyed) reply(socket, { ok: false, error: error.message });
    } finally {
      if (current) {
        active.delete(current);
        current.completed.resolve();
        if (!stopping && !active.size) scheduleIdle();
      }
    }
  }

  async function execute(params, signal) {
    if (params.action === "sessions") {
      const sessions = session ? [session.snapshot()] : [];
      return answer({ sessions }, sessions.length ? `Program ${sessions[0].status}.` : "No debug sessions.");
    }
    if (params.action === "launch") {
      if (session && session.snapshot().status !== "terminated") {
        throw new Error("A debug session is still active. Stop it before launching another.");
      }
      await session?.connection.close();
      session = undefined;
      const launched = new DebugSession(params.program, params.adapter);
      try {
        await launched.launch(params.args, signal);
        session = launched;
        return answer({ snapshot: session.snapshot() }, `Program ${session.snapshot().status}.`);
      } catch (error) {
        // A failed launch can still have started a target. Preserve the launch error while shutting it down.
        await launched.stop().catch(() => {});
        throw error;
      }
    }
    if (!session) throw new Error("No debug session. Launch a program first.");
    return session.execute(params, signal);
  }

  // The foreground launcher holds this pipe. Its exit closes it even on SIGKILL.
  const lease = new Socket({ fd: 3, readable: true, writable: false });
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
    await Promise.all([...active].map(job => job.completed.promise));
    if (request) {
      try {
        const result = session ? await session.stop() : answer({}, "No debug session to terminate.");
        if (!request.socket.destroyed) reply(request.socket, result);
      } catch (error) {
        if (!request.socket.destroyed) reply(request.socket, { ok: false, error: error.message });
      }
    } else {
      await session?.stop();
    }
    await closed;
  } finally {
    lease.destroy();
    process.off("SIGINT", signal);
    process.off("SIGTERM", signal);
    process.off("SIGHUP", signal);
    removeSession(location, original);
  }
}

run().catch(error => {
  console.error(`omp-debug: ${error.message}`);
  process.exitCode = 2;
});
