#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { delimiter, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { checkRenameDestination, checkRenameSource } from "./rename-patch.mjs";

const help = `Usage: omp-lsp [--root DIR] [--json] ACTION [FILE ...]

Actions:
  catalog                        List every built-in OMP language server definition
  status                         Report language servers configured for the project
  check FILE [FILE ...]          Diagnose explicit files using matching servers
  definition FILE --symbol NAME Find definitions, optionally at --line NUMBER
  references FILE --symbol NAME Find references, optionally at --line NUMBER
  hover FILE --line N --symbol NAME
  type-definition FILE --line N --symbol NAME
  implementation FILE --line N --symbol NAME
  symbols FILE                   List document symbols
  symbols --query NAME           Search workspace symbols
  capabilities FILE              Show the matched server's capabilities
  rename-preview FILE --symbol NAME [--line N] --new-name NAME [--patch PATH]
  rename-file-preview FILE --new-name PATH [--patch PATH]

Options: --root DIR, --timeout SECONDS (5–300 per file), --json
Exit status: 0 completed, 1 diagnostics contain errors, 2 incomplete or failed.
Rename previews require a Git worktree. File destinations use a sibling name or project-relative path.
`;

const fileActions = new Set([
  "definition", "references", "hover", "type-definition", "implementation", "symbols", "capabilities",
]);
const prefix = "OMP_CODEX_LSP_RESULT=";
const serverDefaults = {
  "vscode-css-language-server": {
    isLinter: true,
    settings: { css: { validate: true }, scss: { validate: true }, less: { validate: true } },
  },
  "vscode-json-language-server": { isLinter: true, settings: { json: { validate: { enable: true } } } },
};

function positiveInteger(value, name, minimum, maximum) {
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < minimum || number > maximum) {
    throw new Error(`${name} must be an integer from ${minimum} to ${maximum}`);
  }
  return number;
}

function projectFile(root, path) {
  const file = realpathSync(resolve(root, path));
  const name = relative(root, file);
  if (!name || name === ".." || name.startsWith(`..${sep}`) || isAbsolute(name) || !statSync(file).isFile()) {
    throw new Error(`Expected a regular file inside the project: ${path}`);
  }
  return name === "*" ? `.${sep}${name}` : name;
}

function requestsFor(root, action, files, values) {
  const timeout = values.timeout === undefined ? 20 : positiveInteger(values.timeout, "--timeout", 5, 300);
  if (action === "catalog" || action === "status") {
    if (files.length) throw new Error(`${action} takes no file arguments`);
    return [{ action, timeout }];
  }
  if (action === "check") {
    if (files.length === 0 || files.length > 64) throw new Error("check requires 1 to 64 project files");
    return [...new Set(files.map(file => projectFile(root, file)))].map(file => ({ action: "diagnostics", file, timeout }));
  }
  if (action === "rename-preview" || action === "rename-file-preview") {
    const newName = values["new-name"];
    if (files.length !== 1 || !newName?.trim() || (action === "rename-preview" && !values.symbol?.trim())) {
      throw new Error(`${action} requires FILE and --new-name${action === "rename-preview" ? " and --symbol" : ""}`);
    }
    const source = checkRenameSource(root, files[0]).name;
    if (action === "rename-file-preview") checkRenameDestination(root, source, newName);
    const line = values.line === undefined ? undefined : positiveInteger(values.line, "--line", 1, Number.MAX_SAFE_INTEGER);
    return [{ action, file: source, symbol: values.symbol, line, new_name: newName, timeout }];
  }
  if (!fileActions.has(action)) throw new Error(`Unknown action: ${action}`);
  if (action === "symbols" && !files.length && values.query) return [{ action, file: "*", query: values.query, timeout }];
  if (files.length !== 1) throw new Error(`${action} requires one project file`);
  const line = values.line === undefined ? undefined : positiveInteger(values.line, "--line", 1, Number.MAX_SAFE_INTEGER);
  return [{
    action: action.replaceAll("-", "_"), file: projectFile(root, files[0]), line,
    symbol: values.symbol, query: values.query, timeout,
  }];
}

function runOmp(root, requests, patchOutput) {
  const agentDir = realpathSync(mkdtempSync(join(tmpdir(), "omp-codex-lsp-")));
  try {
    const omp = process.env.OMP_LSP_BIN ?? join(homedir(), ".omp", "bin", "19.0.0", "omp");
    const extension = fileURLToPath(new URL("./bridge.mjs", import.meta.url));
    const serverBin = fileURLToPath(new URL("./servers/node_modules/.bin/", import.meta.url));
    const path = [serverBin, dirname(process.execPath), process.env.PATH].filter(Boolean).join(delimiter);
    const catalogueHome = requests[0].action === "catalog" ? { HOME: agentDir } : {};
    // CSS and JSON serve document diagnostics without a workspace-ready event.
    // Supply explicit validation settings for the standalone VS Code servers.
    if (requests[0].action !== "catalog") {
      const servers = { ...serverDefaults };
      // Emmet offers completion only, so a diagnostics request would wait for a report it never sends.
      if (requests[0].action === "diagnostics") servers["emmet-language-server"] = { disabled: true };
      writeFileSync(join(agentDir, "lsp.json"), JSON.stringify({ servers }));
    }
    const processTimeout = requests.reduce((total, request) => total + request.timeout * 1000, 12000);
    const args = [
      "--cwd", root, "--allow-home", "--no-session", "--no-tools", "--no-lsp", "--no-skills", "--no-extensions",
      "--no-rules", "--no-context-files", "--offline", "--no-judge", "--no-spotlight", "-e", extension, "-p",
    ];
    const rename = requests[0].action.startsWith("rename-");
    if (rename && process.platform !== "darwin") throw new Error("Rename preview requires the macOS filesystem sandbox");
    // The native LSP client handles unsolicited workspace/applyEdit requests. Keep the source read-only.
    const policy = `(version 1) (allow default) (deny file-write*) (allow file-write* (subpath ${JSON.stringify(agentDir)}) (literal "/dev/null"))`;
    const scratch = rename ? {
      TMPDIR: agentDir, XDG_CACHE_HOME: join(agentDir, "cache"),
      GOCACHE: join(agentDir, "go-cache"), GOPLSCACHE: join(agentDir, "gopls-cache"),
    } : {};
    const child = spawnSync(rename ? "/usr/bin/sandbox-exec" : omp, rename ? ["-p", policy, omp, ...args] : args, {
      cwd: root,
      env: {
        ...process.env, ...catalogueHome, ...scratch, PATH: path, OMP_PROFILE: "", PI_CONFIG_DIR: "", PI_CODING_AGENT_DIR: agentDir,
        OMP_CODEX_LSP_REQUESTS: JSON.stringify(requests),
      },
      stdio: ["ignore", "pipe", "pipe"],
      encoding: "utf8", maxBuffer: 8 * 1024 * 1024, timeout: processTimeout,
    });
    if (child.error) throw child.error;
    const frames = child.stdout.split(/\r?\n/).filter(line => line.startsWith(prefix));
    if (child.status !== 0 || frames.length !== 1) {
      const detail = child.stderr.trim().slice(-1600);
      throw new Error(`OMP LSP bridge did not complete (${child.signal ?? child.status})${detail ? `\n${detail}` : ""}`);
    }
    const result = JSON.parse(frames[0].slice(prefix.length));
    if (rename && result.exit_code === 0 && result.results[0].patch_bytes) {
      const entry = result.results[0];
      const patch = readFileSync(join(agentDir, "rename.patch"));
      if (patch.length !== entry.patch_bytes) throw new Error("Rename patch is incomplete");
      const path = patchOutput ? resolve(patchOutput) : join(mkdtempSync(join(tmpdir(), "omp-rename-")), "rename.patch");
      writeFileSync(path, patch, { flag: "wx", mode: 0o600 });
      entry.patch_path = path;
      const limit = 8192;
      entry.preview = new TextDecoder().decode(patch.subarray(0, limit), { stream: true });
      if (patch.length > limit) entry.preview += `\n… (${patch.length - limit} more bytes in the full patch)`;
    }
    return result;
  } finally {
    rmSync(agentDir, { recursive: true, force: true });
  }
}

function main() {
  const { values, positionals } = parseArgs({ allowPositionals: true, options: {
    root: { type: "string" }, json: { type: "boolean" }, help: { type: "boolean", short: "h" },
    timeout: { type: "string" }, line: { type: "string" }, symbol: { type: "string" }, query: { type: "string" },
    "new-name": { type: "string" }, patch: { type: "string" },
  } });
  if (values.help || !positionals.length) {
    process.stdout.write(help);
    return;
  }
  const [action, ...files] = positionals;
  if (!action.startsWith("rename-") && (values["new-name"] !== undefined || values.patch !== undefined)) {
    throw new Error("--new-name and --patch are only for rename previews");
  }
  const root = realpathSync(values.root ?? process.cwd());
  if (!statSync(root).isDirectory()) throw new Error(`Expected a project directory: ${root}`);
  const requests = requestsFor(root, action, files, values);
  const result = runOmp(root, requests, values.patch);
  if (values.json) {
    console.log(JSON.stringify(result));
  } else if (result.servers) {
    console.log(`${result.servers.length} OMP language server definitions`);
    for (const server of result.servers) console.log(`${server.name}\t${server.command}\t${server.file_types.join(", ")}`);
  } else {
    for (const entry of result.results) {
      if (entry.file) console.log(`${entry.file}${entry.server ? ` (${entry.server})` : ""}`);
      console.log(entry.text);
      if (entry.patch_path) console.log(`Full patch: ${entry.patch_path}\nApply from: ${entry.apply_root}\n${entry.preview}`);
    }
  }
  process.exitCode = result.exit_code;
}

try {
  main();
} catch (error) {
  console.error(`omp-lsp: ${error.message}`);
  process.exitCode = 2;
}
