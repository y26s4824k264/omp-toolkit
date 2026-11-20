import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
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
  const prefix = join(root, "install with ' spaces");
  const engine = join(root, "engine");
  await mkdir(join(payload, "bin"), { recursive: true });
  for (const name of ["omp-refactor", "omp-shell", "obash", "omp-codex-mcp"]) {
    await writeFile(join(payload, "bin", name), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  }
  for (const [dir, file] of [
    ["lsp", "omp-lsp.mjs"], ["debug", "omp-debug.mjs"], ["stats", "codex-context-stats.mjs"],
    ["profile", "summary.mjs"],
  ]) {
    await mkdir(join(payload, "runtime", dir), { recursive: true });
    await writeFile(join(payload, "runtime", dir, file), "console.log(JSON.stringify(process.argv.slice(2)))\n");
  }
  for (const [file, content] of Object.entries(entries)) await writeFile(join(payload, file), content);
  const tar = run("tar", ["-czf", archive, "-C", payload, "bin", "runtime", ...Object.keys(entries)]);
  assert.equal(tar.status, 0, tar.stderr);
  const digest = createHash("sha256").update(await readFile(archive)).digest("hex");
  await writeFile(checksum, digest + "  toolkit.tar.gz\n");
  await writeFile(engine, "#!/bin/sh\nprintf 'omp/19.0.0\\n'\n", { mode: 0o755 });
  return {
    root, prefix, checksum,
    install: (extra = ["--client", "none"], opts = {}) => run(process.execPath, [
      installer, "--archive", archive, "--checksum", checksum,
      "--version", "v0.2.0", "--prefix", prefix, "--omp-bin", engine, ...extra,
    ], opts),
  };
}

test("installs and re-installs with quoted paths and relative client settings", { skip: windows }, async t => {
  const { prefix, checksum, install } = await fixture(t);
  const installed = install();
  assert.equal(installed.status, 0, installed.stderr);
  const config = JSON.parse(await readFile(join(prefix, "mcp-config.json"), "utf8")).mcpServers.omp;
  assert.equal(config.command, join(prefix, "bin", "omp-codex-mcp"));
  assert.ok(config.args.includes(join(prefix, "runtime", "profile", "summary.mjs")));
  const tool = run(join(prefix, "bin", "codex-context-stats"), ["argument with spaces", "$literal"]);
  assert.equal(tool.status, 0, tool.stderr);
  assert.deepEqual(JSON.parse(tool.stdout), ["argument with spaces", "$literal"]);

  const reinstalled = install();
  assert.equal(reinstalled.status, 0, reinstalled.stderr);
  await writeFile(checksum, "0".repeat(64));
  const corrupt = install();
  assert.equal(corrupt.status, 1);
  assert.match(corrupt.stderr, /SHA-256 verification failed/);
  assert.equal(run(join(prefix, "bin", "codex-context-stats"), []).status, 0);
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
    + 'if [ "$1 $2 $3" = "mcp get omp" ]; then exit 0; fi\n'
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
    ["claude", "No MCP server found with name: omp"],
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
  const binary = join(prefix, "bin", "omp-codex-mcp");
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
