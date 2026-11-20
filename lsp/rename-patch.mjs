import { spawnSync } from "node:child_process";
import { lstatSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

const utf8 = new TextDecoder("utf-8", { fatal: true });
const bom = Buffer.from([0xef, 0xbb, 0xbf]);
const maxFileBytes = 4 * 1024 * 1024;
const maxSnapshotBytes = 32 * 1024 * 1024;

function checkedPath(root, file) {
  const path = resolve(root, file);
  const name = relative(root, path);
  const parts = name.split(sep);
  if (!name || isAbsolute(name) || parts[0] === ".." || parts.some(part => part.toLowerCase() === ".git")) {
    throw new Error(`Expected a path inside the project: ${file}`);
  }
  return { path, name };
}

export function checkRenameSource(root, file) {
  const info = checkedPath(root, file);
  const stat = lstatSync(info.path);
  if (!stat.isFile() || stat.nlink !== 1 || realpathSync(info.path) !== info.path) {
    throw new Error(`Expected a regular project file without symbolic or hard links: ${info.name}`);
  }
  return { ...info, stat };
}

export function checkRenameDestination(root, source, newName) {
  const input = !newName.includes(sep) ? join(dirname(source), newName) : newName;
  const info = checkedPath(root, input);
  const parent = dirname(info.path);
  if (!lstatSync(parent).isDirectory() || realpathSync(parent) !== parent) {
    throw new Error(`Expected an existing destination directory without symbolic links: ${parent}`);
  }
  try {
    lstatSync(info.path);
  } catch (error) {
    if (error.code === "ENOENT") return info;
    throw error;
  }
  throw new Error(`Destination already exists: ${info.name}`);
}

export function readRenameSnapshot(root, file, startedAt) {
  const { path, name, stat } = checkRenameSource(root, file);
  if (stat.size > maxFileBytes) throw new Error(`File exceeds the 4 MiB rename limit: ${name}`);
  if (Math.floor(Math.max(stat.ctimeMs, stat.mtimeMs)) > startedAt) {
    throw new Error(`File changed during language-server lookup: ${name}`);
  }
  const bytes = readFileSync(path);
  if (bytes.length > maxFileBytes || bytes.includes(0)) throw new Error(`Expected a bounded UTF-8 file: ${name}`);
  const mode = stat.mode & 0o100 ? "100755" : "100644";
  return { path, name, bytes, mode, text: utf8.decode(bytes) };
}

export function encodeReplacement(text, original) {
  const bytes = Buffer.from(text, "utf8");
  return original.subarray(0, bom.length).equals(bom) ? Buffer.concat([bom, bytes]) : bytes;
}

function git(args, cwd, input) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
  const result = spawnSync("git", ["-c", "core.quotePath=true", ...args], {
    cwd, input, maxBuffer: 16 * 1024 * 1024, timeout: 15_000,
    env: { ...env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`git ${args[0]} failed: ${result.stderr.toString().trim().slice(-1200)}`);
  return result.stdout;
}

export function renameApplyRoot(root) {
  try {
    const topLevel = git(["rev-parse", "--show-toplevel"], root).toString().replace(/\n$/, "");
    return realpathSync(topLevel);
  } catch (error) {
    throw new Error(`Rename previews need a Git worktree: ${error.message}`);
  }
}

function writeTree(directory, files, knownBlobs) {
  git(["read-tree", "--empty"], directory);
  const entries = files.map(({ name, mode, bytes }) => {
    let id = knownBlobs.get(bytes);
    if (!id) {
      id = git(["hash-object", "--no-filters", "-w", "--stdin"], directory, bytes).toString().trim();
      knownBlobs.set(bytes, id);
    }
    return `${mode} ${id}\t${name}\0`;
  });
  git(["update-index", "-z", "--index-info"], directory, entries.join(""));
  return git(["write-tree"], directory).toString().trim();
}

export function writeRenamePatch(root, applyRoot, agentDir, before, after, moved) {
  if (before.length > 256 || before.reduce((sum, item) => sum + item.bytes.length, 0) > maxSnapshotBytes ||
      after.reduce((sum, item) => sum + item.bytes.length, 0) > maxSnapshotBytes) {
    throw new Error("Rename preview exceeds 256 files or 32 MiB of source");
  }
  const directory = join(agentDir, "rename-git");
  mkdirSync(directory, { mode: 0o700 });
  git(["init", "--bare", "--quiet", "--template="], directory);
  const blobs = new Map();
  const fromGitRoot = file => ({ ...file, name: checkedPath(applyRoot, resolve(root, file.name)).name });
  const oldTree = writeTree(directory, before.map(fromGitRoot), blobs);
  const newTree = writeTree(directory, after.map(fromGitRoot), blobs);
  const patch = git([
    "diff-tree", "--no-commit-id", "-r", "-p", "--binary", "--full-index", "--find-renames",
    "--no-ext-diff", "--no-textconv", "--no-color", "--src-prefix=a/", "--dst-prefix=b/", oldTree, newTree,
  ], directory);
  for (const file of before) {
    const { path, stat } = checkRenameSource(root, file.name);
    if ((stat.mode & 0o100 ? "100755" : "100644") !== file.mode || !readFileSync(path).equals(file.bytes)) {
      throw new Error(`File changed while rendering the patch: ${file.name}`);
    }
  }
  if (moved) checkRenameDestination(root, moved.source, moved.destination);
  if (patch.length) {
    const file = join(agentDir, "rename.patch");
    writeFileSync(file, patch, { flag: "wx", mode: 0o600 });
    git(["apply", "--check", "--", file], applyRoot);
  }
  return patch.length;
}
