#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, createReadStream, createWriteStream } from "node:fs";
import {
  access, chmod, copyFile, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rename, rm, rmdir,
  stat, writeFile,
} from "node:fs/promises";
import { homedir } from "node:os";
import { basename, delimiter, dirname, extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { parseArgs } from "node:util";

const packageName = "omp-toolkit";
const serverName = "omp-toolkit-mcp";
const repo = "y26s4824k264/" + packageName;
const versionPattern = /^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/;
const windows = process.platform === "win32";
const exe = windows ? ".exe" : "";
const marker = ".omp-toolkit-install.json";
const instructionsFile = "AGENT-INSTRUCTIONS.md";
const instructionsStart = "<!-- omp-toolkit:start -->";
const instructionsEnd = "<!-- omp-toolkit:end -->";
const instructions = [
  "## OMP Toolkit",
  "",
  "- For short commands and basic lookups, call the native tools directly. Use `rg` for file and text search and `git` for repository state and diffs.",
  "- For a code question with several keywords and no known file, use `omp-refactor ranked-find \"keywords\"` to rank files and read the matching lines. In a linked Git worktree, set `--root` to that worktree. If the scan times out, narrow the root and retry.",
  "- Prefer the `omp` MCP server for structural search and patch previews. Use `ast_find` for syntax matches, `ast_preview` for rewrite previews, and `hashline_read` / `hashline_preview` for exact-line previews. Inspect the saved full patch and run `git apply --check` before applying. Refresh the preview when source files change.",
  "- For described edits, use `omp-magic prepare`, have your coding agent supply the line operations, then use `omp-magic preview`. A `sub` can insert multiple lines. When preview reports a syntax error, correct the indicated operations and rerun it. Use `omp-refactor sloppy` for text anchors or syntax-aware block edits. Inspect the full Git patch and run `git apply --check` before applying it.",
  "- Run multi-step shell work in a task-specific persistent PTY through `omp-shell`. Use `omp-shell run-log -- COMMAND ARGS...` to retain the original log and exit code for long foreground commands. On macOS and Linux, cancel the owning run-log command so it can clean up its subprocesses. Use the host's job manager for services meant to keep running and `log_minimize` for logs already on disk.",
  "- Use `omp-lsp check` for available language-server diagnostics. Treat missing servers, timeouts, and incomplete reports as unverified. Use `service_ready` for loopback or launch-log checks.",
  "- Use `office_read` for Office text and Excel cell ranges, including coordinates and formulas. Use `image_read` for image metadata or a bounded crop. Render Office pages for visual review.",
  "- When the user asks to retain a project lesson, use `omp-refactor learn save`. Use `omp-refactor learn list` to retrieve lessons. Preview a portable draft with `omp-refactor learn skill preview`, then use `omp-refactor learn skill create` or `omp-refactor learn skill update` when asked to save it.",
  "- For an active long-running goal, recover the user's objective and acceptance criteria before updating the current plan. Keep the current milestone separate. If the user wants project-local tracking, use `omp-refactor goal init`, `goal focus`, and `goal show` with the same project root and goal ID. Record evidence after checking a criterion. Review every criterion against current evidence before completing the host goal.",
  "- For recorded request usage in a Codex rollout log, use `omp-context-stats --turn 1 ROLLOUT.jsonl`. For an offline estimate of a source file, use `token_count` or `omp-refactor token-count`.",
  "",
].join("\n");

const usage = "Usage: node install-release.mjs [--version vX.Y.Z|latest] [--prefix DIR]\n"
  + "       [--client auto|none|codex|claude] [--without-lsp-servers]\n"
  + "       [--archive FILE --checksum FILE] [--portable-node FILE] [--npm-cli FILE]\n";

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8", windowsHide: true, timeout: 60_000, ...options,
  });
}

function checked(command, args, options = {}) {
  const result = run(command, args, options);
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(basename(command) + " failed (" + (result.signal ?? result.status) + ")"
      + (result.stderr?.trim() ? ": " + result.stderr.trim() : ""));
  }
  return result.stdout;
}

async function state(path) {
  try {
    return await lstat(path);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return undefined;
  }
}

function webUrl(url) {
  const parsed = new URL(url);
  if (parsed.protocol !== "https:" || parsed.username || parsed.password) {
    throw new Error("Expected an HTTPS download URL");
  }
  return parsed;
}

async function request(url, timeout = 300_000) {
  let target = webUrl(url);
  const signal = AbortSignal.timeout(timeout);
  for (let redirects = 0; redirects <= 8; redirects++) {
    const response = await fetch(target, {
      redirect: "manual", signal, headers: { "User-Agent": packageName },
    });
    if (response.status >= 300 && response.status < 400 && response.headers.has("location")) {
      target = webUrl(new URL(response.headers.get("location"), target));
      await response.body?.cancel();
      continue;
    }
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error("Download returned HTTP " + response.status + " from " + target.hostname);
    }
    return response;
  }
  throw new Error("Too many download redirects");
}

async function download(url, file) {
  const response = await request(url);
  await pipeline(Readable.fromWeb(response.body), createWriteStream(file, { flags: "wx", mode: 0o600 }));
}

async function verify(file, expected) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  if (hash.digest("hex") !== expected.toLowerCase()) {
    throw new Error("SHA-256 verification failed for " + basename(file));
  }
}

function digestOf(sums, file) {
  const archiveName = basename(file);
  const matches = sums.split(/\r?\n/).map(line => /^([0-9a-f]{64}) [ *](.+)$/i.exec(line))
    .filter(match => match?.[2] === archiveName);
  if (matches.length !== 1) throw new Error("Expected one SHA-256 entry for " + archiveName);
  return matches[0][1];
}

async function artifact(options, temp) {
  if (options.archive) {
    const file = join(temp, basename(options.archive));
    await copyFile(resolve(options.archive), file);
    const sums = (await readFile(resolve(options.checksum), "utf8")).trim();
    const expected = /^[0-9a-f]{64}$/i.test(sums) ? sums : digestOf(sums, file);
    await verify(file, expected);
    return { file, version: options.version === "latest" ? "local" : options.version };
  }
  let version = options.version;
  if (version === "latest") {
    const found = await request("https://github.com/" + repo + "/releases/latest/download/VERSION", 30_000);
    version = (await found.text()).replace(/\r?\n$/, "");
    if (!versionPattern.test(version)) throw new Error("Invalid GitHub release tag");
  }
  const name = packageName + "-" + process.platform + "-" + process.arch + (windows ? ".zip" : ".tar.gz");
  const base = "https://github.com/" + repo + "/releases/download/" + version + "/";
  const sums = await (await request(base + "SHA256SUMS", 30_000)).text();
  const file = join(temp, name);
  const expected = digestOf(sums, file);
  await download(base + name, file);
  await verify(file, expected);
  return { file, version };
}

function extract(file, destination) {
  // Use the Windows system bsdtar, which supports zip archives.
  const tar = windows ? join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe") : "tar";
  const names = checked(tar, ["-tf", file]).trimEnd().split(/\r?\n/);
  const types = checked(tar, ["-tvf", file]).trimEnd().split(/\r?\n/);
  const seen = new Set();
  if (names.length !== types.length) throw new Error("Invalid archive listing");
  for (const [index, name] of names.entries()) {
    const path = name.replace(/^\.\//, "").replace(/\/$/, "");
    if (name === "./" && types[index].startsWith("d")) continue;
    const parts = path.split("/");
    const identity = path.toLowerCase();
    if (!/^[d-]/.test(types[index]) || /[\\:\x00-\x1f]/.test(path)
      || !["bin", "runtime"].includes(parts[0])
      || parts.some(part => !part || part === "." || part === "..")
      || seen.has(identity)) {
      throw new Error("Unexpected entry in toolkit archive");
    }
    seen.add(identity);
  }
  checked(tar, ["-xf", file, "-C", destination]);
}

function within(parent, file) {
  const path = relative(parent, file);
  return path !== "" && path !== ".." && !path.startsWith(".." + sep) && !isAbsolute(path);
}

async function addExecutable(from, to) {
  await copyFile(from, to);
  if (!windows) await chmod(to, 0o755);
}

function shQuote(value) {
  return "'" + value.replaceAll("'", "'\\''") + "'";
}

function psQuote(value) {
  return "'" + value.replaceAll("'", "''") + "'";
}

async function writeLaunchers(stage, prefix, node) {
  const launchers = [
    ["omp-lsp", "lsp/omp-lsp.mjs"],
    ["omp-debug", "debug/omp-debug.mjs"],
    ["omp-magic", "magic/omp-magic.mjs"],
    ["omp-context-stats", "stats/omp-context-stats.mjs"],
  ];
  const dispatcher = [
    'import { fileURLToPath } from "node:url";',
    "const launchers = new Map(" + JSON.stringify(launchers) + ");",
    'if (Number(process.versions.node.split(".")[0]) < 24) throw new Error("Node.js 24 or later is required");',
    "const selected = launchers.get(process.argv[2]);",
    'if (!selected) throw new Error("Unknown toolkit command");',
    "const entry = new URL(selected, import.meta.url);",
    "process.argv.splice(1, 2, fileURLToPath(entry));",
    "await import(entry);",
    "",
  ].join("\n");
  await writeFile(join(stage, "runtime", "launch-cli.mjs"), dispatcher, { flag: "wx" });
  const target = join(prefix, "runtime", "launch-cli.mjs");
  for (const [name] of launchers) {
    const contents = windows
      ? "@echo off\r\nsetlocal DisableDelayedExpansion\r\n"
        + 'set "OMP_TOOLKIT_NODE=%~dp0..\\runtime\\node.exe"\r\n'
        + 'if not exist "%OMP_TOOLKIT_NODE%" set "OMP_TOOLKIT_NODE=node.exe"\r\n'
        + '"%OMP_TOOLKIT_NODE%" "%~dp0..\\runtime\\launch-cli.mjs" ' + name + " %*\r\nexit /b %ERRORLEVEL%\r\n"
      : "#!/bin/sh\nexec " + shQuote(node) + " " + shQuote(target) + " " + name + ' "$@"\n';
    const file = join(stage, "bin", name + (windows ? ".cmd" : ""));
    await writeFile(file, contents, { mode: 0o755, flag: "wx" });
  }
}

async function languageServers(options, stage) {
  if (options["without-lsp-servers"]) return;
  const destination = join(stage, "runtime", "lsp", "servers");
  const args = ["ci", "--ignore-scripts", "--no-audit", "--no-fund"];
  const nodeDirectory = dirname(process.execPath);
  const npmCandidates = options["npm-cli"]
    ? [resolve(options["npm-cli"])]
    : [
      join(nodeDirectory, "node_modules", "npm", "bin", "npm-cli.js"),
      join(dirname(nodeDirectory), "lib", "node_modules", "npm", "bin", "npm-cli.js"),
    ];
  if (windows && !options["npm-cli"]) {
    for (const path of (process.env.PATH ?? "").split(delimiter).filter(isAbsolute)) {
      npmCandidates.push(join(path, "node_modules", "npm", "bin", "npm-cli.js"));
    }
  }
  const npmFiles = await Promise.all(npmCandidates.map(state));
  const npmCli = npmCandidates[npmFiles.findIndex(file => file?.isFile())];
  const runtime = { cwd: destination, stdio: "inherit", timeout: 300_000 };
  if (npmCli) checked(process.execPath, [npmCli, ...args], runtime);
  else if (options["npm-cli"]) throw new Error("npm CLI not found: " + options["npm-cli"]);
  else if (!windows) checked("npm", args, runtime);
  else throw new Error("This install requires npm or --without-lsp-servers");
}

async function protectPrefix(prefix) {
  const before = await state(prefix);
  if (!before) return false;
  if (!before.isDirectory()) throw new Error("The install prefix must be a directory: " + prefix);
  const entries = await readdir(prefix);
  if (entries.length === 0) return true;
  const file = join(prefix, marker);
  const owned = new Set(["bin", "runtime", "mcp-config.json", "vscode-mcp.json", instructionsFile, marker]);
  if (entries.every(entry => owned.has(entry))
    && (await state(file))?.isFile()
    && JSON.parse(await readFile(file, "utf8")).name === packageName) return true;
  throw new Error("This directory is not managed by " + packageName + ". Choose a new --prefix: " + prefix);
}

function clientCommand(client, config) {
  const args = ["--", config.command, ...config.args];
  return client === "codex"
    ? ["mcp", "add", "omp", ...args]
    : ["mcp", "add", "--scope", "user", "--transport", "stdio", "omp", ...args];
}

async function windowsNpmClient(directory, client) {
  if (!(await state(join(directory, client + ".cmd")))?.isFile()) return undefined;
  const packageId = { codex: "@openai/codex", claude: "@anthropic-ai/claude-code" }[client];
  try {
    const root = await realpath(join(directory, "node_modules", ...packageId.split("/")));
    const manifest = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
    const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.[client];
    if (manifest.name !== packageId || typeof bin !== "string" || isAbsolute(bin)) return undefined;
    const file = await realpath(join(root, bin));
    const extension = extname(file).toLowerCase();
    if (!within(root, file) || !(await state(file))?.isFile()) return undefined;
    if (extension === ".exe") return [file];
    if ([".js", ".mjs", ".cjs"].includes(extension)) return [process.execPath, file];
  } catch (error) {
    if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw error;
  }
  return undefined;
}

async function locateClient(client) {
  const workingDirectory = await realpath(process.cwd());
  const paths = (process.env.PATH ?? "").split(delimiter)
    .map(path => windows ? path.replace(/^"(.*)"$/, "$1") : path).filter(isAbsolute);
  for (const path of new Set(paths)) {
    try {
      const directory = await realpath(path);
      // Ignore a checkout accidentally included as a command directory.
      if (relative(workingDirectory, directory) === "") continue;
      const file = join(directory, client + exe);
      if (await state(file) && (await stat(file)).isFile()) {
        if (!windows) await access(file, constants.X_OK);
        return [file];
      }
      if (windows) {
        const npm = await windowsNpmClient(directory, client);
        if (npm) return npm;
      }
    } catch (error) {
      if (!["ENOENT", "EACCES", "EPERM", "ENOTDIR", "ELOOP"].includes(error.code)) throw error;
    }
  }
  return undefined;
}

function clientHome(client, installDirectory) {
  if (client === "codex") {
    return resolve(installDirectory, process.env.CODEX_HOME || join(homedir(), ".codex"));
  }
  return resolve(installDirectory, process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude"));
}

async function instructionPath(client, home) {
  if (client === "claude") return join(home, "CLAUDE.md");
  const override = join(home, "AGENTS.override.md");
  const exists = await state(override);
  if (exists && (!exists.isFile() || (await readFile(override, "utf8")).trim())) return override;
  return join(home, "AGENTS.md");
}

function managedInstructions(previous) {
  const starts = [...previous.matchAll(/^<!-- omp-toolkit:start -->\r?$/gm)];
  const ends = [...previous.matchAll(/^<!-- omp-toolkit:end -->\r?$/gm)];
  const eol = previous.includes("\r\n") ? "\r\n" : "\n";
  let block = [instructionsStart, instructions.trimEnd(), instructionsEnd].join("\n").replace(/\n/g, eol);
  if (!starts.length && !ends.length) {
    let separator = previous && !previous.endsWith(eol + eol) ? eol : "";
    if (previous && !previous.endsWith(eol)) separator += eol;
    return previous + separator + block + eol;
  }
  if (starts.length !== 1 || ends.length !== 1 || starts[0].index >= ends[0].index) {
    throw new Error("The existing OMP Toolkit instruction markers are incomplete or duplicated");
  }
  if (ends[0][0].endsWith("\r")) block += "\r";
  return previous.slice(0, starts[0].index) + block + previous.slice(ends[0].index + ends[0][0].length);
}

async function writeManagedInstructions(file) {
  const original = await state(file);
  if (original && !original.isFile()) throw new Error("The global instruction path is not a regular file");
  const originalBytes = original ? await readFile(file) : Buffer.alloc(0);
  const previous = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(originalBytes);
  const updated = managedInstructions(previous);
  if (updated === previous) return;
  const directory = dirname(file);
  await mkdir(directory, { recursive: true });
  const temp = await mkdtemp(join(directory, ".omp-instructions-"));
  const staged = join(temp, "instructions.md");
  try {
    await writeFile(staged, updated, { mode: 0o600 });
    if (original && !windows) await chmod(staged, original.mode & 0o777);
    const current = await state(file);
    if (Boolean(original) !== Boolean(current)
      || (current && (!current.isFile() || current.mode !== original.mode || current.ino !== original.ino
        || !(await readFile(file)).equals(originalBytes)))) {
      throw new Error("The global instruction file changed while it was being updated");
    }
    await rename(staged, file);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

function matchesClientEntry(entry, config) {
  return entry?.command === config.command && (entry.type === undefined || entry.type === "stdio")
    && Array.isArray(entry.args) && entry.args.length === config.args.length
    && entry.args.every((arg, index) => arg === config.args[index]);
}

async function clientInstructions(client, directory, config, result, explicit) {
  try {
    const home = clientHome(client, directory);
    let entry;
    if (client === "codex") {
      const found = JSON.parse(result.stdout);
      if (found.enabled !== false) entry = found.transport;
    } else {
      const file = process.env.CLAUDE_CONFIG_DIR ? join(home, ".claude.json") : join(homedir(), ".claude.json");
      if (await state(file)) entry = JSON.parse(await readFile(file, "utf8")).mcpServers?.omp;
    }
    if (!matchesClientEntry(entry, config)) return;
    const file = await instructionPath(client, home);
    await writeManagedInstructions(file);
    console.log("OMP Toolkit guidance: " + file);
  } catch (error) {
    console.error("Could not update " + client + " instructions: " + error.message);
    console.error("Copy the guidance from " + join(directory, instructionsFile));
    if (explicit) process.exitCode = 1;
  }
}

async function configureClient(client, config, directory, explicit) {
  const quote = windows ? psQuote : shQuote;
  const add = clientCommand(client, config);
  const fallback = () => console.log(client + " " + add.map(value => value.startsWith("--")
    || /^[a-z]+$/.test(value) ? value : quote(value)).join(" "));
  let command;
  try {
    command = await locateClient(client);
  } catch {
    console.error("Could not check for " + client + ". Use the generated MCP configuration or run:");
    fallback();
    if (explicit) process.exitCode = 1;
    return true;
  }
  if (!command) {
    if (explicit) {
      console.error(client + " was not found on PATH. To connect it after installing the client, run:");
      fallback();
      process.exitCode = 1;
    }
    return false;
  }
  const [binary, ...entry] = command;
  const lookup = client === "codex" ? ["mcp", "get", "omp", "--json"] : ["mcp", "get", "omp"];
  const result = run(binary, [...entry, ...lookup], { cwd: directory, timeout: 12_000 });
  if (result.status === 0) {
    console.log(client + " already has an 'omp' entry. It has been left in place.");
    await clientInstructions(client, directory, config, result, explicit);
    return true;
  }
  const absent = client === "codex"
    ? /No MCP server named ['"]omp['"] found\./
    : /^(?:Error:\s*)?No MCP server (?:named|found with name:?)\s+["']?omp["']?(?:[.\s]|$)/im;
  if (!result.error && result.status === 1 && absent.test(result.stderr + "\n" + result.stdout)) {
    const added = run(binary, [...entry, ...add], { cwd: directory, stdio: "inherit", timeout: 30_000 });
    if (added.status === 0) {
      console.log("Connected omp to " + client + ".");
      const installed = client === "codex"
        ? run(binary, [...entry, ...lookup], { cwd: directory, timeout: 12_000 }) : result;
      if (client !== "codex" || installed.status === 0) {
        await clientInstructions(client, directory, config, installed, explicit);
      }
      return true;
    }
  }
  console.error("Could not connect " + client + " automatically. Use the generated MCP configuration or run:");
  fallback();
  if (explicit) process.exitCode = 1;
  return true;
}

function defaultPrefix() {
  return windows
    ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData", "Local"), "OMP Toolkit")
    : join(homedir(), ".local", packageName);
}

async function install(options) {
  if (!["darwin", "linux", "win32"].includes(process.platform) || !["x64", "arm64"].includes(process.arch)) {
    throw new Error("Unsupported platform or architecture");
  }
  if (options.version !== "latest" && !versionPattern.test(options.version)) throw new Error("Invalid --version");
  if (!["auto", "none", "codex", "claude"].includes(options.client)) throw new Error("Invalid --client");
  if (options["with-lsp-servers"] && options["without-lsp-servers"]) throw new Error("Choose one LSP installation mode");
  if (Boolean(options.archive) !== Boolean(options.checksum)) throw new Error("--archive and --checksum must be used together");
  const requested = resolve(options.prefix ?? defaultPrefix());
  if (requested === dirname(requested)) throw new Error("The install prefix must not be a filesystem root");
  await mkdir(dirname(requested), { recursive: true });
  const parent = await realpath(dirname(requested));
  const prefix = join(parent, basename(requested));
  const lock = prefix + ".install-lock";
  try {
    await mkdir(lock, { mode: 0o700 });
  } catch (error) {
    if (error.code !== "EEXIST") throw error;
    throw new Error("Installation already in progress: " + lock);
  }
  let temp;
  try {
    const replace = await protectPrefix(prefix);
    temp = await mkdtemp(join(parent, "." + packageName + "-"));
    const stage = join(temp, "payload");
    await mkdir(stage, { mode: 0o700 });
    const { file, version } = await artifact(options, temp);
    extract(file, stage);
    for (const name of ["omp-refactor", "omp-shell", "obash", serverName]) {
      if (!(await state(join(stage, "bin", name + exe)))?.isFile()) throw new Error("Archive is missing " + name);
    }
    for (const module of [
      "lsp/omp-lsp.mjs", "debug/omp-debug.mjs", "magic/omp-magic.mjs",
      "stats/omp-context-stats.mjs", "profile/summary.mjs",
    ]) {
      if (!(await state(join(stage, "runtime", module)))?.isFile()) throw new Error("Archive is missing " + module);
    }

    let node = await realpath(process.execPath);
    if (options["portable-node"] || within(prefix, node)) {
      const portable = join("runtime", "node" + exe);
      const source = options["portable-node"] ? resolve(options["portable-node"]) : node;
      const version = checked(source, ["--version"], { timeout: 10_000 }).trim();
      if (!/^v(?:2[4-9]|[3-9]\d|\d{3,})\./.test(version)) throw new Error("Portable Node.js 24 or later is required");
      await addExecutable(source, join(stage, portable));
      node = join(prefix, portable);
    }
    await languageServers(options, stage);
    await writeLaunchers(stage, prefix, node);

    const bin = join(prefix, "bin");
    const config = {
      command: join(bin, serverName + exe),
      args: ["--bin-dir", bin, "--node", node,
        "--profile-helper", join(prefix, "runtime", "profile", "summary.mjs")],
    };
    await writeFile(join(stage, "mcp-config.json"), JSON.stringify({ mcpServers: { omp: config } }, null, 2) + "\n");
    await writeFile(join(stage, "vscode-mcp.json"), JSON.stringify({
      servers: { omp: { type: "stdio", ...config } },
    }, null, 2) + "\n");
    await writeFile(join(stage, instructionsFile), instructions);
    await writeFile(join(stage, marker), JSON.stringify({ name: packageName, version }) + "\n");

    const backup = join(temp, "previous");
    if (replace) await rename(prefix, backup);
    try {
      await rename(stage, prefix);
    } catch (error) {
      try {
        if (replace) await rename(backup, prefix);
      } catch {
        console.error("Previous install is preserved at " + backup);
        temp = undefined; // Keep the recoverable previous installation.
      }
      throw error;
    }

    console.log("Installed " + packageName + " " + version + " in " + prefix);
    console.log("MCP configuration: " + join(prefix, "mcp-config.json"));
    console.log("VS Code MCP configuration: " + join(prefix, "vscode-mcp.json"));
    console.log("Agent instructions: " + join(prefix, instructionsFile));
    console.log(windows
      ? "$env:Path = " + psQuote(bin + ";") + " + $env:Path"
      : "export PATH=" + shQuote(bin) + ':"$PATH"');
    if (options.client === "auto") {
      const codex = await configureClient("codex", config, prefix, false);
      const claude = await configureClient("claude", config, prefix, false);
      if (!codex && !claude) console.log("Use the generated MCP configuration to connect your client.");
    } else if (options.client !== "none") {
      await configureClient(options.client, config, prefix, true);
    }
  } finally {
    try {
      if (temp) await rm(temp, { recursive: true, force: true, maxRetries: 2 });
    } catch (error) {
      console.error("Staging files remain at " + temp + ": " + error.message);
    } finally {
      await rmdir(lock);
    }
  }
}

try {
  const { values } = parseArgs({ options: {
    version: { type: "string", default: "latest" },
    prefix: { type: "string" },
    client: { type: "string", default: "auto" },
    "portable-node": { type: "string" },
    "npm-cli": { type: "string" },
    "with-lsp-servers": { type: "boolean" },
    "without-lsp-servers": { type: "boolean" },
    archive: { type: "string" },
    checksum: { type: "string" },
    help: { type: "boolean", short: "h" },
  } });
  if (values.help) process.stdout.write(usage);
  else {
    if (Number(process.versions.node.split(".")[0]) < 24) throw new Error("Node.js 24 or later is required");
    await install(values);
  }
} catch (error) {
  console.error("Install failed: " + error.message);
  process.exitCode = 1;
}
