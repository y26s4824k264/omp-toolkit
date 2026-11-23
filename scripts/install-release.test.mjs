import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const installer = fileURLToPath(new URL("./install-release.mjs", import.meta.url));
const windows = process.platform === "win32";

function run(command, args, options = {}) {
  return spawnSync(command, args, { encoding: "utf8", ...options });
}

async function fixture(t, entries = {}) {
  const root = await realpath(await mkdtemp(join(tmpdir(), "omp toolkit test ")));
  t.after(() => rm(root, { recursive: true, force: true }));
  const payload = join(root, "payload");
  const archive = join(root, "toolkit.tar.gz");
  const checksum = join(root, "SHA256SUMS");
  const prefix = join(root, "install with ' spaces and 中文");
  await mkdir(join(payload, "bin"), { recursive: true });
  for (const name of ["omp-refactor", "omp-shell", "obash", "omp-toolkit-mcp"]) {
    await writeFile(join(payload, "bin", name), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  }
  for (const [dir, file] of [
    ["lsp", "omp-lsp.mjs"], ["debug", "omp-debug.mjs"], ["stats", "omp-context-stats.mjs"],
    ["profile", "summary.mjs"], ["magic", "omp-magic.mjs"],
  ]) {
    await mkdir(join(payload, "runtime", dir), { recursive: true });
    await writeFile(join(payload, "runtime", dir, file), "console.log(JSON.stringify(process.argv.slice(2)))\n");
  }
  const servers = join(payload, "runtime", "lsp", "servers");
  await mkdir(servers, { recursive: true });
  await writeFile(join(servers, "package.json"), '{"name":"fixture-language-servers"}\n');
  await writeFile(join(servers, "package-lock.json"), '{"name":"fixture-language-servers","lockfileVersion":3}\n');
  for (const [file, content] of Object.entries(entries)) await writeFile(join(payload, file), content);
  const tar = run("tar", ["-czf", archive, "-C", payload, "bin", "runtime", ...Object.keys(entries)]);
  assert.equal(tar.status, 0, tar.stderr);
  const digest = createHash("sha256").update(await readFile(archive)).digest("hex");
  await writeFile(checksum, digest + "  toolkit.tar.gz\n");
  return {
    root, prefix, checksum,
    install: (extra = ["--client", "none"], opts = {}) => run(process.execPath, [
      installer, "--archive", archive, "--checksum", checksum,
      "--version", "v0.2.0", "--prefix", prefix,
      ...(extra.includes("--npm-cli") ? [] : ["--without-lsp-servers"]), ...extra,
    ], opts),
  };
}

test("installs and re-installs with quoted paths and relative client settings", { skip: windows }, async t => {
  const { prefix, checksum, install } = await fixture(t);
  const installed = install();
  assert.equal(installed.status, 0, installed.stderr);
  const config = JSON.parse(await readFile(join(prefix, "mcp-config.json"), "utf8")).mcpServers.omp;
  assert.equal(config.command, join(prefix, "bin", "omp-toolkit-mcp"));
  assert.deepEqual(config.args, [
    "--bin-dir", join(prefix, "bin"), "--node", await realpath(process.execPath),
    "--profile-helper", join(prefix, "runtime", "profile", "summary.mjs"),
  ]);
  await assert.rejects(lstat(join(prefix, "bin", "omp-codex-mcp")), { code: "ENOENT" });
  const tool = run(join(prefix, "bin", "omp-context-stats"), ["argument with spaces", "$literal"]);
  assert.equal(tool.status, 0, tool.stderr);
  assert.deepEqual(JSON.parse(tool.stdout), ["argument with spaces", "$literal"]);
  const lsp = run(join(prefix, "bin", "omp-lsp"), ["catalog"]);
  assert.equal(lsp.status, 0, lsp.stderr);
  assert.deepEqual(JSON.parse(lsp.stdout), ["catalog"]);

  const reinstalled = install();
  assert.equal(reinstalled.status, 0, reinstalled.stderr);
  await writeFile(checksum, "0".repeat(64));
  const corrupt = install();
  assert.equal(corrupt.status, 1);
  assert.match(corrupt.stderr, /SHA-256 verification failed/);
  assert.equal(run(join(prefix, "bin", "omp-context-stats"), []).status, 0);
});

test("protects existing directories and rejects unrelated archive entries", { skip: windows }, async t => {
  const { prefix, install } = await fixture(t, { unrelated: "outside the payload" });
  await mkdir(prefix);
  await writeFile(join(prefix, "keep.txt"), "keep this file");
  const unmanaged = install();
  assert.equal(unmanaged.status, 1);
  assert.match(unmanaged.stderr, /not managed/);
  assert.equal(await readFile(join(prefix, "keep.txt"), "utf8"), "keep this file");

  const invalid = install(["--prefix", prefix + "-different"]);
  assert.equal(invalid.status, 1);
  assert.match(invalid.stderr, /Unexpected entry/);
});

test("reuses an existing Codex MCP name", { skip: windows }, async t => {
  const { root, install } = await fixture(t);
  await writeFile(join(root, "codex"), "#!/bin/sh\n"
    + 'if [ "$1 $2 $3" = "mcp get omp" ]; then printf "%s\\n" \'{"transport":{"command":"/other/omp"}}\'; exit 0; fi\n'
    + "exit 5\n", { mode: 0o755 });
  const result = install(["--client", "codex"], {
    env: { ...process.env, PATH: root + ":" + process.env.PATH },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /already has an 'omp' entry/);
});

test("connects both available clients by default using explicit stdio commands", { skip: windows }, async t => {
  const { root, prefix, install } = await fixture(t);
  for (const [name, absent] of [
    ["codex", "Error: No MCP server named 'omp' found."],
    ["claude", 'No MCP server named "omp". Run the add command to add one.'],
  ]) {
    await writeFile(join(root, name), "#!/bin/sh\n"
      + 'if [ "$1 $2 $3" = "mcp get omp" ]; then\n'
      + "  printf '%s\\n' " + JSON.stringify(absent) + " >&2\n"
      + "  exit 1\n"
      + "fi\n"
      + 'printf "%s\\n" "$@" > "$OMP_INSTALL_TEST_LOG/' + name + '.log"\n', { mode: 0o755 });
  }
  const result = install([], {
    env: { ...process.env, OMP_INSTALL_TEST_LOG: root, PATH: root + ":/usr/bin:/bin" },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Connected omp to codex/);
  assert.match(result.stdout, /Connected omp to claude/);
  const codex = await readFile(join(root, "codex.log"), "utf8");
  const claude = await readFile(join(root, "claude.log"), "utf8");
  const binary = join(prefix, "bin", "omp-toolkit-mcp");
  assert.ok(codex.startsWith("mcp\nadd\nomp\n--\n" + binary + "\n"));
  assert.ok(claude.startsWith("mcp\nadd\n--scope\nuser\n--transport\nstdio\nomp\n--\n" + binary + "\n"));
});

test("installs without clients and keeps guidance on an explicit missing client", { skip: windows }, async t => {
  const { prefix, install } = await fixture(t);
  const options = { env: { ...process.env, PATH: "/usr/bin:/bin" } };
  const automatic = install([], options);
  assert.equal(automatic.status, 0, automatic.stderr);
  assert.match(automatic.stdout, /Use the generated MCP configuration/);
  const explicit = install(["--client", "claude"], options);
  assert.equal(explicit.status, 1);
  assert.match(explicit.stderr, /claude was not found on PATH/);
  assert.ok(await readFile(join(prefix, "mcp-config.json"), "utf8"));
});

test("installs the locked language servers by default", { skip: windows }, async t => {
  const { root, prefix, install } = await fixture(t);
  const npm = join(root, "npm-cli.js");
  await writeFile(npm, [
    'const { readFileSync, writeFileSync, mkdirSync } = require("node:fs");',
    'if (process.argv.slice(2).join(" ") !== "ci --ignore-scripts --no-audit --no-fund") process.exit(7);',
    'const lock = JSON.parse(readFileSync("package-lock.json", "utf8"));',
    'mkdirSync("node_modules");',
    'writeFileSync("node_modules/installed", lock.name);',
  ].join("\n"));
  const result = install(["--client", "none", "--npm-cli", npm]);
  assert.equal(result.status, 0, result.stderr);
  const stamp = join(prefix, "runtime", "lsp", "servers", "node_modules", "installed");
  assert.equal(await readFile(stamp, "utf8"), "fixture-language-servers");
  assert.match(await readFile(join(prefix, "AGENT-INSTRUCTIONS.md"), "utf8"), /git apply --check/);
});

test("adds and updates only the managed instructions of matching clients", { skip: windows }, async t => {
  const { root, prefix, install } = await fixture(t);
  const plain = install();
  assert.equal(plain.status, 0, plain.stderr);
  const config = JSON.parse(await readFile(join(prefix, "mcp-config.json"), "utf8")).mcpServers.omp;
  const codex = join(root, "codex-config");
  const claude = join(root, "claude-config");
  await mkdir(codex);
  await mkdir(claude);
  const agents = join(codex, "AGENTS.md");
  const override = join(codex, "AGENTS.override.md");
  const md = join(claude, "CLAUDE.md");
  const defaultRule = "# User fallback rules\nDo not touch.\n";
  const privateRule = "# User global rules\r\nKeep these instructions.\r\n";
  await writeFile(agents, defaultRule);
  await writeFile(override, privateRule);
  await writeFile(md, privateRule);
  await writeFile(join(codex, "mock-mcp.json"), JSON.stringify({
    enabled: true, transport: { type: "stdio", ...config },
  }));
  await writeFile(join(claude, ".claude.json"), JSON.stringify({ mcpServers: { omp: config } }));
  await writeFile(join(root, "codex"), '#!/bin/sh\n[ "$2" = get ] || exit 5\ncat "$CODEX_HOME/mock-mcp.json"\n',
    { mode: 0o755 });
  await writeFile(join(root, "claude"), '#!/bin/sh\n[ "$2" = get ] || exit 5\nexit 0\n', { mode: 0o755 });
  const opts = { env: {
    ...process.env, PATH: root + ":/usr/bin:/bin", CODEX_HOME: codex, CLAUDE_CONFIG_DIR: claude,
  } };
  const first = install([], opts);
  assert.equal(first.status, 0, first.stderr);
  const firstCodex = await readFile(override, "utf8");
  const firstClaude = await readFile(md, "utf8");
  for (const file of [firstCodex, firstClaude]) {
    assert.ok(file.startsWith(privateRule + "\r\n"));
    assert.equal(file.match(/<!-- omp-toolkit:start -->/g).length, 1);
    assert.match(file, /<!-- omp-toolkit:end -->\r\n$/);
  }
  assert.equal(await readFile(agents, "utf8"), defaultRule);
  const retry = install([], opts);
  assert.equal(retry.status, 0, retry.stderr);
  assert.equal(await readFile(override, "utf8"), firstCodex);
  assert.equal(await readFile(md, "utf8"), firstClaude);

  await writeFile(md, privateRule);
  await writeFile(join(claude, ".claude.json"), JSON.stringify({
    mcpServers: { omp: { ...config, command: "/other/omp" } },
  }));
  const oldServer = install([], opts);
  assert.equal(oldServer.status, 0, oldServer.stderr);
  assert.equal(await readFile(md, "utf8"), privateRule);
});
