import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import { languageId } from "./servers.mjs";

const maxPayload = 8 * 1024 * 1024;
const maxDocument = 4 * 1024 * 1024;
const posix = process.platform !== "win32";
const acknowledgedRequests = new Set(["window/workDoneProgress/create", "window/showMessageRequest"]);

function sectionValue(settings, section) {
  if (!section) return settings;
  return section.split(".").reduce((value, key) => value && Object.hasOwn(value, key) ? value[key] : null, settings);
}

export class LspClient {
  #child;
  #closed;
  #failure;
  #buffer = Buffer.alloc(0);
  #bodyLength;
  #nextId = 0;
  #pending = new Map();
  #diagnostics = new Map();
  #stderr = "";
  #progress = new Set();
  #registered = new Map();
  openFiles = new Map();
  capabilities = {};

  constructor(server, scratch) {
    this.server = server;
    this.rootUri = pathToFileURL(server.root).href;
    this.workspaceFolders = [{ uri: this.rootUri, name: server.root }];
    let { command, args } = server.binary;
    let env = process.env;
    if (scratch) {
      // On macOS, language-server extensions cannot change the project during preview.
      // The server can only write its own scratch subtree, never the patch output.
      const serverScratch = join(scratch, "server");
      mkdirSync(serverScratch, { recursive: true, mode: 0o700 });
      const writable = mkdtempSync(join(serverScratch, "lsp-"));
      const policy = "(version 1) (allow default) (deny file-write*) (allow file-write* (subpath "
        + JSON.stringify(writable) + ") (literal \"/dev/null\"))";
      args = ["-p", policy, command, ...args];
      command = "/usr/bin/sandbox-exec";
      env = {
        ...env, TMPDIR: writable, XDG_CACHE_HOME: join(writable, "cache"),
        GOCACHE: join(writable, "go-cache"), GOPLSCACHE: join(writable, "gopls-cache"),
      };
    }
    this.#child = spawn(command, args, {
      cwd: server.root, env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true, detached: posix,
    });
    this.#closed = new Promise(resolve => this.#child.once("close", (code, signal) => {
      this.#fail(new Error(server.name + " exited (" + (signal ?? code) + ")"));
      resolve();
    }));
    this.#child.on("error", error => this.#fail(error));
    this.#child.stdin.on("error", error => this.#fail(error));
    this.#child.stderr.setEncoding("utf8").on("data", chunk => {
      this.#stderr = (this.#stderr + chunk).slice(-2000);
    });
    this.#child.stdout.on("data", chunk => {
      try {
        this.#receive(chunk);
      } catch (error) {
        this.#fail(error);
        this.#signal("SIGTERM");
      }
    });
  }

  async initialize(signal) {
    const reply = await this.request("initialize", {
      processId: process.pid, rootPath: this.server.root, rootUri: this.rootUri,
      workspaceFolders: this.workspaceFolders,
      clientInfo: { name: "OMP Toolkit" },
      initializationOptions: this.server.initOptions,
      capabilities: {
        general: { positionEncodings: ["utf-16"] },
        window: { workDoneProgress: true },
        workspace: {
          applyEdit: false, configuration: true, workspaceFolders: true,
          workspaceEdit: { documentChanges: true },
          fileOperations: { dynamicRegistration: true, willRename: true },
        },
        textDocument: {
          synchronization: { didSave: false },
          diagnostic: {},
          publishDiagnostics: { relatedInformation: true },
          hover: { contentFormat: ["markdown", "plaintext"] },
          documentSymbol: { hierarchicalDocumentSymbolSupport: true },
        },
      },
    }, signal);
    this.capabilities = reply?.capabilities ?? {};
    if (this.capabilities.positionEncoding && this.capabilities.positionEncoding !== "utf-16") {
      throw new Error(this.server.name + " uses unsupported position encoding " + this.capabilities.positionEncoding);
    }
    this.notify("initialized", {});
    this.notify("workspace/didChangeConfiguration", { settings: this.server.settings });
  }

  #fail(error) {
    if (this.#failure) return;
    this.#failure = this.#stderr ? new Error(error.message + "\n" + this.#stderr.trim()) : error;
    for (const request of this.#pending.values()) request.reject(this.#failure);
    this.#pending.clear();
  }

  #receive(chunk) {
    this.#buffer = Buffer.concat([this.#buffer, chunk]);
    for (;;) {
      if (this.#bodyLength === undefined) {
        const end = this.#buffer.indexOf("\r\n\r\n");
        if (end < 0 && this.#buffer.length < 8192) return;
        if (end < 0 || end >= 8192) throw new Error("LSP response header exceeds the limit");
        const value = /^Content-Length:\s*(\d+)\s*$/im.exec(this.#buffer.toString("ascii", 0, end))?.[1];
        this.#bodyLength = Number(value);
        if (!Number.isSafeInteger(this.#bodyLength) || this.#bodyLength <= 0 || this.#bodyLength > maxPayload) {
          throw new Error("Invalid or oversized LSP response");
        }
        this.#buffer = this.#buffer.subarray(end + 4);
      }
      if (this.#buffer.length < this.#bodyLength) return;
      const message = JSON.parse(this.#buffer.toString("utf8", 0, this.#bodyLength));
      this.#buffer = this.#buffer.subarray(this.#bodyLength);
      this.#bodyLength = undefined;
      this.#dispatch(message);
    }
  }

  #dispatch(message) {
    const { id, method, params } = message;
    if (id !== undefined && method !== undefined) {
      let result = null;
      if (method === "workspace/configuration") {
        result = (params?.items ?? []).map(item => sectionValue(this.server.settings, item.section));
      } else if (method === "workspace/workspaceFolders") {
        result = this.workspaceFolders;
      } else if (method === "workspace/applyEdit") {
        result = { applied: false, failureReason: "OMP Toolkit only previews edits" };
      } else if (method === "client/registerCapability") {
        for (const item of params?.registrations ?? []) this.#registered.set(item.id, item.method);
      } else if (method === "client/unregisterCapability") {
        for (const item of params?.unregisterations ?? params?.unregistrations ?? []) this.#registered.delete(item.id);
      } else if (!acknowledgedRequests.has(method)) {
        this.#send({ id, error: { code: -32601, message: "Unsupported client method " + method } });
        return;
      }
      this.#send({ id, result });
    } else if (id !== undefined) {
      const pending = this.#pending.get(id);
      if (pending) {
        this.#pending.delete(id);
        if (message.error) pending.reject(Object.assign(new Error(message.error.message), { code: message.error.code }));
        else pending.resolve(message.result);
      }
    } else if (method === "textDocument/publishDiagnostics" && Array.isArray(params?.diagnostics)) {
      this.#diagnostics.set(params.uri, { items: params.diagnostics, at: Date.now() });
    } else if (method === "$/progress") {
      if (params?.value?.kind === "begin") this.#progress.add(params.token);
      if (params?.value?.kind === "end") this.#progress.delete(params.token);
    }
  }

  #send(payload) {
    if (this.#failure) throw this.#failure;
    const body = JSON.stringify({ jsonrpc: "2.0", ...payload });
    if (Buffer.byteLength(body) > maxPayload) throw new Error("LSP request exceeds the size limit");
    this.#child.stdin.write("Content-Length: " + Buffer.byteLength(body) + "\r\n\r\n" + body);
  }

  request(method, params, signal) {
    signal.throwIfAborted();
    const id = ++this.#nextId;
    const pending = Promise.withResolvers();
    const abort = () => {
      if (this.#pending.delete(id)) {
        pending.reject(new Error(method + " timed out for " + this.server.name));
        if (!this.#failure) {
          try { this.notify("$/cancelRequest", { id }); } catch {}
        }
      }
    };
    this.#pending.set(id, pending);
    signal.addEventListener("abort", abort, { once: true });
    try {
      this.#send({ id, method, params });
    } catch (error) {
      this.#pending.delete(id);
      pending.reject(error);
    }
    return pending.promise.finally(() => signal.removeEventListener("abort", abort));
  }

  notify(method, params) {
    this.#send({ method, params });
  }

  open(file) {
    const uri = pathToFileURL(file).href;
    const existing = this.openFiles.get(uri);
    if (existing) return existing;
    if (statSync(file).size > maxDocument) throw new Error("Language-server document exceeds 4 MiB");
    const raw = readFileSync(file);
    if (raw.length > maxDocument || raw.includes(0)) throw new Error("Expected a bounded UTF-8 file");
    const text = new TextDecoder("utf-8", { fatal: true }).decode(raw);
    const document = { uri, languageId: languageId(this.server, file), version: 1, text };
    this.notify("textDocument/didOpen", { textDocument: document });
    this.openFiles.set(uri, document);
    return document;
  }

  async diagnostics(file, signal) {
    const document = this.open(file);
    if (this.capabilities.diagnosticProvider) {
      try {
        const reply = await this.request("textDocument/diagnostic", { textDocument: { uri: document.uri } }, signal);
        if (Array.isArray(reply?.items)) return reply.items;
      } catch (error) {
        if (error.code !== -32601) throw error;
      }
    }
    for (;;) {
      signal.throwIfAborted();
      if (this.#failure) throw this.#failure;
      const report = this.#diagnostics.get(document.uri);
      if (report && Date.now() - report.at >= 175) return report.items;
      await delay(40, undefined, { signal });
    }
  }

  hasCapability(method) {
    return [...this.#registered.values()].includes(method);
  }

  async waitForIndex(signal, file) {
    if (file && this.server.name === "typescript-language-server") await this.diagnostics(file, signal);
    while (this.#progress.size) {
      signal.throwIfAborted();
      if (this.#failure) throw this.#failure;
      await delay(60, undefined, { signal });
    }
  }

  #signal(signal) {
    if (posix && this.#child.pid !== undefined) {
      try {
        // A POSIX server has its own process group, including server workers.
        process.kill(-this.#child.pid, signal);
        return;
      } catch (error) {
        if (error.code !== "ESRCH") this.#fail(error);
      }
    }
    this.#child.kill(signal);
  }

  async close() {
    if (!this.#failure) {
      try {
        await this.request("shutdown", null, AbortSignal.timeout(1500));
        this.notify("exit");
      } catch {}
    }
    this.#child.stdin.end();
    const stopped = () => Promise.race([this.#closed.then(() => true), delay(350, false, { ref: false })]);
    if (await stopped()) return;
    this.#signal("SIGTERM");
    if (await stopped()) return;
    this.#signal("SIGKILL");
    if (await stopped()) return;
    // A detached or Windows server worker can outlive its parent with inherited pipes.
    this.#child.stdin.destroy();
    this.#child.stdout.destroy();
    this.#child.stderr.destroy();
    this.#child.unref();
  }
}

export function symbolPosition(text, spec, line) {
  const lines = text.split("\n");
  if (line !== undefined && line > lines.length) throw new Error("Line is outside the document");
  if (!spec && line !== undefined) return { line: line - 1, character: Math.max(0, lines[line - 1].search(/\S/)) };
  if (!spec) throw new Error("--symbol or --line is required");
  const numbered = /^(.+)#(\d+)$/.exec(spec);
  const symbol = numbered?.[1] ?? spec;
  const occurrence = numbered ? Math.max(1, Number(numbered[2])) : 1;
  const escaped = RegExp.escape(symbol);
  const pattern = /^[$_\p{ID_Start}][$_\p{ID_Continue}]*$/u.test(symbol)
    ? "(?<![\\p{ID_Continue}$])" + escaped + "(?![\\p{ID_Continue}$])" : escaped;
  const start = line === undefined ? 0 : line - 1;
  const stop = line === undefined ? lines.length : line;
  for (const flags of ["gu", "giu"]) {
    const expression = new RegExp(pattern, flags);
    let seen = 0;
    for (let index = start; index < stop; index++) {
      for (const match of lines[index].matchAll(expression)) {
        if (++seen === occurrence) return { line: index, character: match.index };
      }
    }
    if (seen) break;
  }
  throw new Error("Symbol or occurrence not found: " + spec);
}
