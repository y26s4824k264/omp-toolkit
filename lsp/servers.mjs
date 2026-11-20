import { constants, accessSync, existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { basename, delimiter, dirname, extname, isAbsolute, join, matchesGlob, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const defaults = JSON.parse(readFileSync(new URL("./defaults.json", import.meta.url), "utf8"));
const installedBin = fileURLToPath(new URL("./servers/node_modules/.bin/", import.meta.url));
const windows = process.platform === "win32";
const requiresMarkers = new Set(["biome", "denols", "eslint", "helm-ls", "tailwindcss"]);
const localSettings = {
  "vscode-css-language-server": { css: { validate: true }, scss: { validate: true }, less: { validate: true } },
  "vscode-json-language-server": { json: { validate: { enable: true } } },
};

export function catalogue() {
  return Object.entries(defaults).sort(([a], [b]) => a.localeCompare(b)).map(([name, server]) => ({
    name, command: server.command, file_types: server.fileTypes, root_markers: server.rootMarkers,
  }));
}

export function serverDefinitions(root) {
  const definitions = Object.fromEntries(Object.entries(defaults).map(([name, server]) => [
    name, { ...server, settings: { ...server.settings, ...localSettings[name] } },
  ]));
  for (const name of [".omp/lsp.json", ".lsp.json", "lsp.json"]) {
    const file = join(root, name);
    if (!existsSync(file)) continue;
    const config = JSON.parse(readFileSync(file, "utf8"));
    for (const [key, override] of Object.entries(config.servers ?? {})) {
      if (!override || typeof override !== "object" || Array.isArray(override)) {
        throw new Error("Invalid language server configuration in " + file + ": " + key);
      }
      const known = Object.hasOwn(definitions, key) ? definitions[key] : undefined;
      Object.defineProperty(definitions, key, {
        value: { ...known, ...override, settings: { ...known?.settings, ...override.settings } },
        enumerable: true, configurable: true, writable: true,
      });
    }
  }
  return Object.entries(definitions).filter(([, server]) => !server.disabled).map(([name, server]) => ({
    ...server, name, args: server.args ?? [], fileTypes: server.fileTypes ?? [], rootMarkers: server.rootMarkers ?? [],
  }));
}

function ancestors(root, path) {
  const directories = [];
  for (let current = path;; current = dirname(current)) {
    directories.push(current);
    if (current === root || dirname(current) === current) return directories;
  }
}

function markerFound(directory, marker) {
  if (!/[*?[\]{}]/.test(marker)) return existsSync(join(directory, marker));
  return readdirSync(directory).some(name => matchesGlob(name, marker));
}

function executable(file) {
  try {
    accessSync(file, windows ? constants.F_OK : constants.X_OK);
    return statSync(file).isFile();
  } catch {
    return false;
  }
}

function npmWindowsScript(shim) {
  const directory = dirname(shim);
  if (basename(directory).toLowerCase() !== ".bin") return;
  // npm's cmd-shim launches JavaScript through node; resolve the same file
  // directly so paths and arguments never pass through cmd.exe.
  const contents = readFileSync(shim, "utf8");
  if (!/\bSET "_prog=node(?:\.exe)?"/i.test(contents)) return;
  const quoted = /"%_prog%"\s+"%dp0%\\([^"\r\n]+)" %\*/i.exec(contents)?.[1];
  if (!quoted) return;
  const file = resolve(directory, quoted);
  if (!executable(file)) return;
  if (/\.[cm]?js$/i.test(file) || /^#![^\r\n]*\bnode(?:\.exe)?(?:[ \t]|\r?\n)/.test(readFileSync(file, "utf8"))) return file;
}

function resolvedCommand(server, directories) {
  const { command, args } = server;
  if (typeof command !== "string" || !command || !Array.isArray(args) || !args.every(arg => typeof arg === "string")) {
    throw new Error("Invalid command or arguments for language server " + server.name);
  }
  const commandArgs = args.map(arg => arg === "$PID" ? String(process.pid) : arg);
  const searchPath = [
    ...directories.map(path => join(path, "node_modules", ".bin")),
    installedBin,
    ...(process.env.PATH ?? "").split(delimiter).filter(isAbsolute),
  ];
  const candidates = isAbsolute(command) ? [command] : command.includes("/") || command.includes("\\")
    ? [resolve(directories.at(-1), command)] : searchPath.map(directory => join(directory, command));

  for (const file of candidates) {
    const native = windows && !extname(file) ? file + ".exe" : file;
    if (windows && /\.(?:cmd|bat|ps1)$/i.test(native)) {
      const script = executable(native) ? npmWindowsScript(native) : undefined;
      if (script) return { command: process.execPath, args: [script, ...commandArgs] };
    } else if (executable(native)) {
      return windows && /\.[cm]?js$/i.test(native)
        ? { command: process.execPath, args: [native, ...commandArgs] }
        : { command: native, args: commandArgs };
    }
    if (windows && !extname(file) && executable(file + ".cmd")) {
      const script = npmWindowsScript(file + ".cmd");
      if (script) return { command: process.execPath, args: [script, ...commandArgs] };
    }
  }
}

function matchesFile(server, file) {
  const lower = basename(file).toLowerCase();
  return server.fileTypes.some(name => name.startsWith(".")
    ? lower.endsWith(name.toLowerCase()) : lower === name.toLowerCase());
}

export function workspaceDocument(server) {
  const directories = [server.root];
  const ignored = new Set(["node_modules", "target", "dist", "build", "vendor"]);
  for (let index = 0; index < directories.length; index++) {
    const directory = directories[index];
    for (const item of readdirSync(directory, { withFileTypes: true })) {
      if (item.isFile() && !item.name.endsWith(".d.ts") && matchesFile(server, item.name)) {
        return join(directory, item.name);
      }
      if (directories.length < 256 && item.isDirectory() && !item.name.startsWith(".") && !ignored.has(item.name)) {
        directories.push(join(directory, item.name));
      }
    }
  }
}

function supportsStdio(server) {
  // The upstream catalog also contains a standalone linter and a separate native TypeScript preview.
  return !(server.name === "swiftlint" && server.command === "swiftlint"
    || server.name === "typescript-native" && server.command === "tsc");
}

export function selectServers(definitions, root, file, action) {
  const directories = ancestors(root, file ? dirname(file) : root);
  return definitions.flatMap(server => {
    if (!supportsStdio(server) || (file && !matchesFile(server, file))) return [];
    if (action === "diagnostics" && server.name === "emmet-language-server") return [];
    const marker = directories.findIndex(path => server.rootMarkers.some(name => markerFound(path, name)));
    if (marker < 0 && (!file || requiresMarkers.has(server.name))) return [];
    const binary = resolvedCommand(server, directories);
    return binary ? [{ ...server, binary, root: directories[marker] ?? root, marker }] : [];
  }).sort((a, b) =>
    (a.marker < 0 ? Infinity : a.marker) - (b.marker < 0 ? Infinity : b.marker)
    || Number(Boolean(a.isLinter)) - Number(Boolean(b.isLinter)));
}

export function serverStatus(definitions, root) {
  return definitions.map(server => ({
    name: server.name, command: server.command, file_types: server.fileTypes, root_markers: server.rootMarkers,
    available: supportsStdio(server) && Boolean(resolvedCommand(server, [root])),
    matches_root: server.rootMarkers.some(name => markerFound(root, name)),
  }));
}

export function languageId(server, file) {
  const extension = extname(file).toLowerCase().slice(1);
  const ids = {
    ts: "typescript", tsx: "typescriptreact", js: "javascript", jsx: "javascriptreact",
    mjs: "javascript", cjs: "javascript", py: "python", pyi: "python",
    rb: "ruby", rs: "rust", sh: "shellscript", bash: "shellscript", zsh: "shellscript",
    yml: "yaml", md: "markdown", h: "c", hpp: "cpp", cc: "cpp", cxx: "cpp", cs: "csharp",
  };
  return server.languageId ?? ids[extension] ?? (extension || basename(file).toLowerCase());
}
