#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, createReadStream, createWriteStream } from "node:fs";
import {
  access, chmod, copyFile, cp, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rename, rm, rmdir,
  stat, symlink, writeFile,
} from "node:fs/promises";
import { homedir } from "node:os";
import { basename, delimiter, dirname, extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { parseArgs } from "node:util";

const packageName = "omp-toolkit";
const serverName = "omp-toolkit-mcp";
const repo = "y26s4824k264/" + packageName;
const upstreamVersion = "19.0.0";
const upstreamCommit = "57771ae562b9345e0579bceb2d68d585e3e6927e";
const platforms = { darwin: "macos", linux: "linux", win32: "windows" };
const arches = { x64: "x86_64", arm64: "arm64" };
const versionPattern = /^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/;
const windows = process.platform === "win32";
const exe = windows ? ".exe" : "";
const marker = ".omp-toolkit-install.json";

const usage = "Usage: node install-release.mjs [--version vX.Y.Z|latest] [--prefix DIR]\n"
  + "       [--client auto|none|codex|claude] [--omp-bin FILE] [--with-lsp-servers]\n"
  + "       [--archive FILE --checksum FILE] [--portable-node FILE]\n";

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
    const found = await request("https://api.github.com/repos/" + repo + "/releases/latest", 30_000);
    version = (await found.json()).tag_name;
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

function knownOmp(file) {
  if (!isAbsolute(file)) return false;
  const result = run(file, ["--version"], { timeout: 10_000 });
  return result.status === 0 && result.stdout.trim() === "omp/" + upstreamVersion;
}

function within(parent, file) {
  const path = relative(parent, file);
  return path !== "" && path !== ".." && !path.startsWith(".." + sep) && !isAbsolute(path);
}

async function engine(options, stage, prefix) {
  const binary = "omp" + exe;
  const relativeDestination = join("runtime", binary);
  const managed = join(prefix, relativeDestination);
  const candidates = options["omp-bin"] ? [resolve(options["omp-bin"])] : [
    managed,
    join(homedir(), ".omp", "bin", upstreamVersion, binary),
    join(homedir(), ".omp", "bin", binary),
    join(homedir(), ".local", "bin", binary),
    ...(process.env.PATH ?? "").split(delimiter).filter(isAbsolute).map(path => join(path, binary)),
  ];
  const selected = candidates.find(knownOmp);
  if (selected) {
    const file = await realpath(selected);
    if (!within(prefix, file)) return file;
    await addExecutable(file, join(stage, relativeDestination));
    return managed;
  }
  if (options["omp-bin"]) throw new Error("--omp-bin must point to a working Oh My Pi " + upstreamVersion);

  const host = platforms[process.platform] + "-" + arches[process.arch];
  const url = "https://build.stencil.so/api/products/omp/versions/" + upstreamVersion + "/" + host;
  const manifest = await (await request(url, 30_000)).json();
  const expectedFile = "omp-" + process.platform.replace("win32", "windows") + "-" + process.arch + exe;
  if (manifest.build?.version !== upstreamVersion || manifest.build.commit !== upstreamCommit
    || manifest.file?.name !== expectedFile || !/^[0-9a-f]{64}$/i.test(manifest.file.sha256)) {
    throw new Error("Oh My Pi build manifest does not match the pinned release");
  }
  const file = join(stage, relativeDestination);
  await download(manifest.download, file);
  if (!windows) await chmod(file, 0o755);
  await verify(file, manifest.file.sha256);
  if (!knownOmp(file)) throw new Error("The downloaded Oh My Pi binary could not start");
  return managed;
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
    ["omp-lsp", ["lsp/omp-lsp.mjs", "OMP_LSP_BIN"]],
    ["omp-debug", ["debug/omp-debug.mjs", "OMP_DEBUG_BIN"]],
    ["codex-context-stats", ["stats/codex-context-stats.mjs"]],
  ];
  const dispatcher = [
    'import { readFileSync } from "node:fs";',
    'import { fileURLToPath } from "node:url";',
    "const launchers = new Map(" + JSON.stringify(launchers) + ");",
    'if (Number(process.versions.node.split(".")[0]) < 24) throw new Error("Node.js 24 or later is required");',
    "const selected = launchers.get(process.argv[2]);",
    'if (!selected) throw new Error("Unknown toolkit command");',
    "const [modulePath, variable] = selected;",
    "if (variable && !process.env[variable]) {",
    '  const config = JSON.parse(readFileSync(new URL("../mcp-config.json", import.meta.url), "utf8"));',
    "  const args = config.mcpServers.omp.args;",
    '  const index = args.indexOf("--omp");',
    '  if (index < 0 || !args[index + 1]) throw new Error("Missing Oh My Pi path in the install configuration");',
    "  process.env[variable] = args[index + 1];",
    "}",
    "const entry = new URL(modulePath, import.meta.url);",
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

async function languageServers(options, prefix, stage) {
  const relativeServers = join("runtime", "lsp", "servers");
  const destination = join(stage, relativeServers);
  const previous = join(prefix, relativeServers);
  const args = ["ci", "--ignore-scripts", "--no-audit", "--no-fund"];
  if (options["with-lsp-servers"]) {
    const nodeDirectory = dirname(process.execPath);
    const npmCandidates = [
      join(nodeDirectory, "node_modules", "npm", "bin", "npm-cli.js"),
      join(dirname(nodeDirectory), "lib", "node_modules", "npm", "bin", "npm-cli.js"),
    ];
    const npmFiles = await Promise.all(npmCandidates.map(state));
    const npmCli = npmCandidates[npmFiles.findIndex(file => file?.isFile())];
    if (npmCli) checked(process.execPath, [npmCli, ...args], { cwd: destination, stdio: "inherit", timeout: 300_000 });
    else if (!windows) checked("npm", args, { cwd: destination, stdio: "inherit", timeout: 300_000 });
    else throw new Error("--with-lsp-servers requires npm from an installed Node.js distribution");
    return;
  }
  const existing = join(previous, "node_modules");
  if (!(await state(existing))?.isDirectory()) return;
  const oldLock = join(previous, "package-lock.json");
  if ((await state(oldLock))?.isFile()
    && (await readFile(oldLock)).equals(await readFile(join(destination, "package-lock.json")))) {
    await cp(existing, join(destination, "node_modules"), {
      recursive: true, verbatimSymlinks: true, force: false, errorOnExist: true,
    });
    return;
  }
  console.error("The optional language-server lockfile changed. Re-run with --with-lsp-servers to update.");
}

async function protectPrefix(prefix) {
  const before = await state(prefix);
  if (!before) return false;
  if (!before.isDirectory()) throw new Error("The install prefix must be a directory: " + prefix);
  const entries = await readdir(prefix);
  if (entries.length === 0) return true;
  const file = join(prefix, marker);
  const owned = new Set(["bin", "runtime", "mcp-config.json", "vscode-mcp.json", marker]);
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
    return true;
  }
  const absent = client === "codex"
    ? /No MCP server named ['"]omp['"] found\./
    : /^(?:Error:\s*)?No MCP server (?:named|found with name:?)\s+["']?omp["']?(?:[.\s]|$)/im;
  if (!result.error && result.status === 1 && absent.test(result.stderr + "\n" + result.stdout)) {
    const added = run(binary, [...entry, ...add], { cwd: directory, stdio: "inherit", timeout: 30_000 });
    if (added.status === 0) {
      console.log("Connected omp to " + client + ".");
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
  if (!platforms[process.platform] || !arches[process.arch]) throw new Error("Unsupported platform or architecture");
  if (options.version !== "latest" && !versionPattern.test(options.version)) throw new Error("Invalid --version");
  if (!["auto", "none", "codex", "claude"].includes(options.client)) throw new Error("Invalid --client");
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
      "lsp/omp-lsp.mjs", "debug/omp-debug.mjs", "stats/codex-context-stats.mjs", "profile/summary.mjs",
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
    const omp = await engine(options, stage, prefix);
    await languageServers(options, prefix, stage);
    await writeLaunchers(stage, prefix, node);
    if (!windows) await symlink(serverName, join(stage, "bin", "omp-codex-mcp"));

    const bin = join(prefix, "bin");
    const config = {
      command: join(bin, serverName + exe),
      args: ["--bin-dir", bin, "--omp", omp, "--node", node,
        "--profile-helper", join(prefix, "runtime", "profile", "summary.mjs")],
    };
    await writeFile(join(stage, "mcp-config.json"), JSON.stringify({ mcpServers: { omp: config } }, null, 2) + "\n");
    await writeFile(join(stage, "vscode-mcp.json"), JSON.stringify({
      servers: { omp: { type: "stdio", ...config } },
    }, null, 2) + "\n");
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
    "omp-bin": { type: "string" },
    "portable-node": { type: "string" },
    "with-lsp-servers": { type: "boolean" },
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
