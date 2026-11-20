import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("./omp-lsp.mjs", import.meta.url));

function run(root, ...args) {
  return spawnSync(process.execPath, [cli, "--root", root, "--json", ...args], { encoding: "utf8" });
}

test("lists all pinned upstream server definitions without a project", () => {
  const result = run(tmpdir(), "catalog");
  assert.equal(result.status, 0, result.stderr);
  const names = JSON.parse(result.stdout).servers.map(server => server.name);
  assert.equal(new Set(names).size, 55);
  for (const name of ["gopls", "clangd", "rust-analyzer", "pyright", "typescript-language-server", "zls"]) {
    assert.ok(names.includes(name), `${name} is missing`);
  }
});

test("an existing bracketed path with no matching server is incomplete", () => {
  const root = mkdtempSync(join(tmpdir(), "omp-lsp-test-"));
  try {
    mkdirSync(join(root, "[id]"));
    writeFileSync(join(root, "[id]", "fixture.unknown"), "fixture\n");
    const result = run(root, "check", "[id]/fixture.unknown");
    assert.equal(result.status, 2, result.stderr);
    const { results: [report] } = JSON.parse(result.stdout);
    assert.equal(report.file, "[id]/fixture.unknown");
    assert.equal(report.status, "incomplete");
    assert.match(report.text, /No language server handles/);
  } finally {
    rmSync(root, { recursive: true });
  }
});

test("a server that ignores shutdown and TERM cannot hang the CLI", () => {
  const root = mkdtempSync(join(tmpdir(), "omp-lsp-stubborn-"));
  const pidFile = join(root, "server.pid");
  try {
    const script = join(root, "server.cjs");
    writeFileSync(script, [
      'require("node:fs").writeFileSync("server.pid", String(process.pid));',
      'process.on("SIGTERM", () => {});',
      'setInterval(() => {}, 1000);',
      'process.stdin.once("data", () => {',
      '  const reply = JSON.stringify({jsonrpc:"2.0",id:1,result:{capabilities:{hoverProvider:true}}});',
      '  process.stdout.write("Content-Length: " + Buffer.byteLength(reply) + "\\r\\n\\r\\n" + reply);',
      '});',
      'process.stdin.resume();',
    ].join("\n"));
    writeFileSync(join(root, "example.fixture"), "fixture");
    writeFileSync(join(root, "lsp.json"), JSON.stringify({
      servers: { fixture: { command: process.execPath, args: [script], fileTypes: [".fixture"] } },
    }));
    const result = spawnSync(process.execPath, [cli, "--root", root, "--json", "capabilities", "example.fixture"], {
      encoding: "utf8", timeout: 7000,
    });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 0, result.stdout || result.stderr);
    assert.match(JSON.parse(result.stdout).results[0].text, /"hoverProvider": true/);
  } finally {
    if (existsSync(pidFile)) {
      try { process.kill(Number(readFileSync(pidFile, "utf8")), "SIGKILL"); } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    }
    rmSync(root, { recursive: true });
  }
});

const localBin = fileURLToPath(new URL("./servers/node_modules/.bin/", import.meta.url));
const serverPaths = [localBin, ...(process.env.PATH ?? "").split(delimiter)];
const serverExists = name => serverPaths.some(directory => process.platform === "win32"
  ? [".exe", ".cmd"].some(suffix => existsSync(join(directory, name + suffix)))
  : existsSync(join(directory, name)));
const hasValidationServers = ["vscode-json-language-server", "vscode-css-language-server"]
  .every(serverExists);

test("malformed JSON and CSS report diagnostics without workspace indexing", { skip: !hasValidationServers }, () => {
  const root = mkdtempSync(join(tmpdir(), "omp-lsp-validation-test-"));
  try {
    writeFileSync(join(root, "package.json"), "{}\n");
    writeFileSync(join(root, "bad.json"), '{"item": }\n');
    writeFileSync(join(root, "bad.css"), "main { color: red\n");
    const result = run(root, "--timeout", "5", "check", "bad.json", "bad.css");
    assert.equal(result.status, 1, result.stdout || result.stderr);
    const { results: [json, css] } = JSON.parse(result.stdout);
    assert.equal(json.status, "errors");
    assert.match(json.text, /Value expected/);
    assert.equal(css.status, "errors");
    assert.match(css.text, /css-semicolonexpected/);
  } finally {
    rmSync(root, { recursive: true });
  }
});

const hasTypeScript = serverExists("typescript-language-server");

test("direct TypeScript stdio finds cross-file references and workspace symbols", { skip: !hasTypeScript }, () => {
  const root = mkdtempSync(join(tmpdir(), "omp-lsp-project-"));
  try {
    writeFileSync(join(root, "tsconfig.json"), '{"include":["*.ts"]}\n');
    writeFileSync(join(root, "lib.ts"), "export function exactSymbol(input: number) { return input }\n");
    writeFileSync(join(root, "app.ts"), 'import { exactSymbol } from "./lib"; console.log(exactSymbol(13));\n');
    function query(...arguments_) {
      const result = run(root, ...arguments_);
      assert.equal(result.status, 0, result.stdout || result.stderr);
      const { results: [report] } = JSON.parse(result.stdout);
      assert.equal(report.server, "typescript-language-server");
      return JSON.parse(report.text);
    }
    const definition = query("definition", "app.ts", "--symbol", "exactSymbol#2");
    assert.ok(definition.some(hit => (hit.uri ?? hit.targetUri).endsWith("/lib.ts")));
    const references = query("references", "lib.ts", "--symbol", "exactSymbol");
    assert.ok(references.some(hit => hit.uri.endsWith("/app.ts")));
    assert.match(JSON.stringify(query("hover", "lib.ts", "--line", "1", "--symbol", "exactSymbol")), /exactSymbol/);
    assert.ok(query("symbols", "lib.ts").some(item => item.name === "exactSymbol"));
    assert.ok(query("symbols", "--query", "exactSymbol").some(item => item.name === "exactSymbol"));
  } finally {
    rmSync(root, { recursive: true });
  }
});

test("TypeScript rename previews produce complete Git patches from a nested LSP root", {
  skip: process.platform !== "darwin" || !hasTypeScript,
}, () => {
  const fixture = mkdtempSync(join(tmpdir(), "omp-lsp-rename-test-"));
  try {
    const repo = join(fixture, "repo");
    const root = join(repo, "packages", "sample");
    mkdirSync(root, { recursive: true });
    const git = (...args) => {
      const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
    };
    git("init", "--quiet", "--template=");
    const files = {
      "tsconfig.json": '{"compilerOptions":{"jsx":"react-jsx"}}\n',
      "lib.tsx": "export function oldName() { return 1 }\n",
      "usage.ts": 'import { oldName } from "./lib.js";\r\nconst marker = "🧪"; console.log(oldName());\r\n',
      "unrelated.ts": "function oldName() { return 2 }\n",
    };
    for (const [file, content] of Object.entries(files)) writeFileSync(join(root, file), content);

    const preview = (output, ...args) => {
      const file = join(fixture, output);
      const result = run(root, ...args, "--patch", file);
      assert.equal(result.status, 0, result.stderr || result.stdout);
      const { results: [report] } = JSON.parse(result.stdout);
      assert.equal(report.patch_path, file);
      assert.equal(report.apply_root, realpathSync(repo));
      const patch = readFileSync(file, "utf8");
      assert.equal(report.patch_bytes, Buffer.byteLength(patch));
      git("apply", "--check", "--", file);
      return patch;
    };

    const symbol = preview("symbol.patch", "rename-preview", "lib.tsx", "--symbol", "oldName", "--line", "1", "--new-name", "newName");
    assert.match(symbol, /\+export function newName\(/);
    assert.match(symbol, /\+import \{ newName \}/);
    assert.match(symbol, /\+const marker = "🧪"; console\.log\(newName\(\)\);\r\n/);
    assert.doesNotMatch(symbol, /unrelated\.ts/);

    const file = preview("move.patch", "rename-file-preview", "lib.tsx", "--new-name", "renamed file.tsx");
    assert.match(file, /rename from packages\/sample\/lib\.tsx\nrename to packages\/sample\/renamed file\.tsx/);
    assert.match(file, /\+import \{ oldName \} from "\.\/renamed file\.js"/);
    assert.equal(existsSync(join(root, "renamed file.tsx")), false);
    for (const [path, content] of Object.entries(files)) assert.equal(readFileSync(join(root, path), "utf8"), content);

    git("apply", "--", join(fixture, "move.patch"));
    assert.equal(existsSync(join(root, "lib.tsx")), false);
    assert.equal(readFileSync(join(root, "renamed file.tsx"), "utf8"), files["lib.tsx"]);
    assert.equal(readFileSync(join(root, "usage.ts"), "utf8"), files["usage.ts"].replace("./lib.js", "./renamed file.js"));
  } finally {
    rmSync(fixture, { recursive: true });
  }
});
