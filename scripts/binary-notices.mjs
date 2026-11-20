import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, readdirSync,
  realpathSync, writeFileSync,
} from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const toolkit = realpathSync(fileURLToPath(new URL("..", import.meta.url)));
const read = (file) => readFileSync(file, "utf8");
const fallback = JSON.parse(read(new URL("binary-notices.fallback.json", import.meta.url)));
const permissive = new Set([
  "0BSD", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "BSL-1.0", "bzip2-1.0.6",
  "CC0-1.0", "ISC", "MIT", "MIT-0", "Unicode-3.0",
  "Unlicense", "Zlib",
]);
const selectedLicenses = new Map([
  ["self_cell@1.3.0", ["Apache-2.0 OR GPL-2.0-only", "Apache-2.0"]],
]);
const noticeName = /^(licen[cs]es?|copying|copyright|notice|unlicense|authors)(?:$|[-_.])/i;
const licenseName = /^(licen[cs]e|copying|unlicense)(?:$|[-_.])/i;
const sourceExtension = /\.(?:rs|js|ts|c|h|cpp)$/i;

function check(condition, message) {
  if (!condition) throw new Error(message);
}

function within(root, path) {
  const rel = relative(root, path);
  return !isAbsolute(rel) && rel !== ".." && !rel.startsWith(".." + sep);
}

function pathsFromBuild(file, targets) {
  const messages = read(file).split(/\r?\n/u)
    .filter((line) => line.startsWith("{")).map((line) => JSON.parse(line));
  check(messages.at(-1)?.reason === "build-finished" &&
    messages.at(-1).success === true, "Missing successful Cargo build in " + file);
  const artifacts = messages.filter((message) => message.reason === "compiler-artifact");
  for (const target of targets) {
    check(artifacts.some((artifact) => artifact.target.name === target && artifact.executable),
      "Cargo build log is missing executable " + target);
  }
  return artifacts;
}

// Published crates use Cargo's normalized package manifests. Workspace license inheritance
// for the two local source trees is resolved against their own workspace manifests.
function manifestSection(file, heading) {
  const text = read(file);
  const start = text.split(/\r?\n/u).findIndex((line) => line.trim() === heading);
  check(start >= 0, "Missing " + heading + " in " + file);
  return text.split(/\r?\n/u).slice(start + 1).join("\n").split(/^\s*\[/mu)[0];
}

function stringField(section, name) {
  const line = section.split("\n").find((line) => line.trimStart().startsWith(name + " ="));
  if (!line) return null;
  const quoted = line.slice(line.indexOf("=") + 1).trim();
  check(quoted.startsWith('"'), "Expected Cargo's normalized string field for " + name);
  return JSON.parse(quoted.match(/^"(?:[^"\\]|\\.)*"/u)[0]);
}

function licenseSelection(label, license) {
  const reviewed = selectedLicenses.get(label);
  if (reviewed) {
    check(license === reviewed[0], label + " changed its license declaration");
    return reviewed[1];
  }
  if (license === "MPL-2.0") return license;
  const terms = license.replaceAll("Apache-2.0 WITH LLVM-exception", "Apache-2.0")
    .replace(/[()/]/gu, " ").trim().split(/\s+/u);
  check(terms.some((term) => permissive.has(term)) &&
    terms.every((term) => permissive.has(term) || /^(AND|OR)$/u.test(term)),
    label + " has an unreviewed license declaration: " + license);
  return license;
}

function noticeFiles(dir, recursive) {
  return readdirSync(dir, { recursive, withFileTypes: true })
    .filter((entry) => entry.isFile() || entry.isSymbolicLink())
    .map((entry) => join(entry.parentPath, entry.name))
    .filter((file) => {
      const rel = relative(dir, file).split(sep);
      const name = basename(file);
      return !sourceExtension.test(name) && (noticeName.test(name) || rel[0] === "LICENSES");
    }).sort();
}

function crateArchive(dir, label) {
  // Cargo extracts registry/src/INDEX/NAME-VERSION from registry/cache/INDEX/NAME-VERSION.crate.
  const index = dirname(dir);
  const registry = dirname(dirname(index));
  const file = join(registry, "cache", basename(index), basename(dir) + ".crate");
  check(existsSync(file), label + " requires its original Cargo source archive");
  return file;
}

function copyNotice(source, destination) {
  const clean = realpathSync(source);
  check(readFileSync(clean).length > 0, "Empty notice " + source);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(clean, destination);
}

function registryOrigin(label) {
  const [name, version] = label.split("@");
  return "https://crates.io/crates/" + name + "/" + version;
}

function capturePackage(artifact, vendorRoots) {
  const manifest = realpathSync(artifact.manifest_path);
  const owner = [...vendorRoots.entries()].find(([, root]) => within(root, manifest));
  const registry = artifact.package_id.startsWith("registry+https://github.com/rust-lang/crates.io-index#");
  check(owner || registry, "Unsupported dependency source " + artifact.package_id);
  const category = owner?.[0] ?? "crates.io";
  const ownerRoot = owner?.[1];
  const section = manifestSection(manifest, "[package]");
  const name = stringField(section, "name");
  const version = artifact.package_id.split("#").at(-1).split("@").at(-1);
  const declaredVersion = stringField(section, "version");
  check(/^[A-Za-z0-9_-]+$/u.test(name ?? "") && /^\d[\w.+-]*$/u.test(version ?? ""),
    "Invalid Cargo package identity for " + manifest);
  check(!declaredVersion || declaredVersion === version, "Manifest changed since building " + name);
  const label = name + "@" + version;
  const dir = dirname(manifest);
  const license = stringField(section, "license") ?? (ownerRoot &&
    stringField(manifestSection(join(ownerRoot, "Cargo.toml"), "[workspace.package]"), "license"));
  const explicit = stringField(section, "license-file");
  check(license, label + " has no SPDX license declaration");
  const selection = licenseSelection(label, license);
  const files = noticeFiles(dir, dir !== ownerRoot);
  const declaration = explicit ? resolve(dir, explicit) : null;
  const packageLicense = files.some((file) => {
    const local = relative(dir, file).split(sep);
    return (local.length === 1 && licenseName.test(local[0])) || local[0] === "LICENSES";
  });
  const additional = fallback[label];
  if (additional) {
    const vcs = JSON.parse(read(join(dir, ".cargo_vcs_info.json")));
    check(vcs.git?.sha1 === additional.revision, label + " has changed source revision");
  }
  check(packageLicense || declaration || ownerRoot || additional,
    label + " is missing its license text. Record the exact upstream source in the fallback manifest.");
  return {
    category, label, origin: ownerRoot ? relative(ownerRoot, manifest).split(sep).join("/") : registryOrigin(label),
    license, selection, dir, ownerRoot, declaration, files, additional,
  };
}

function writePackage(item, output) {
  const {
    category, label, dir, ownerRoot, declaration, files, additional, license, selection, origin,
  } = item;
  const folder = join("cargo-notices", category, label);
  const destination = join(output, folder);
  if (declaration && !files.includes(declaration)) files.push(declaration);
  for (const file of files) {
    const resolved = realpathSync(file);
    check(within(ownerRoot ?? dir, resolved), label + " has a notice outside its source tree");
    const rel = relative(dir, file);
    const path = within(dir, file) ? rel : join("declared", basename(file));
    copyNotice(file, join(destination, path));
  }
  if (additional) {
    mkdirSync(destination, { recursive: true });
    for (const licenseFile of additional.files ?? [additional]) {
      const name = licenseFile.name ?? "UPSTREAM-LICENSE";
      check(/^[A-Za-z0-9_.-]+$/u.test(name) && name !== "." && name !== "..",
        "Invalid stored license name for " + label);
      writeFileSync(join(destination, name), licenseFile.text);
    }
  }
  if (ownerRoot && !existsSync(destination)) {
    copyNotice(join(ownerRoot, "LICENSE"), join(destination, "WORKSPACE-LICENSE"));
  }

  const links = [];
  if (additional) {
    for (const file of additional.files ?? [additional]) links.push("[original notice](" + file.source + ")");
  }
  if (selection === "MPL-2.0") {
    const source = crateArchive(dir, label);
    const published = basename(source);
    copyNotice(source, join(destination, published));
    const hash = createHash("sha256").update(readFileSync(source)).digest("hex");
    writeFileSync(join(destination, "SOURCE.md"),
      "# Source Code Form\n\n" + label + " is distributed under the Mozilla Public License 2.0.\n\n" +
      "The original unchanged crates.io source archive is [" + published + "](" + published + ").\n\n" +
      "Original release: " + origin + "\n\nSHA-256: " + hash + "\n");
    links.push("[MPL source](" + join(folder, "SOURCE.md").split(sep).join("/") + ")");
  }
  if (selection !== license) links.push("distributed under " + selection);
  return "| " + label + " | " + category + " | " + license + " | " + origin + " | " +
    "[Notices](" + folder.split(sep).join("/") + "/)" +
    (links.length ? " · " + links.join(" · ") : "") + " |";
}

function copyRustRuntime(output) {
  const sysroot = execFileSync("rustc", ["--print", "sysroot"], {
    cwd: toolkit,
    encoding: "utf8",
  }).trim();
  const docs = join(sysroot, "share", "doc", "rust");
  const target = join(output, "rust-standard-library");
  copyNotice(join(docs, "COPYRIGHT-library.html"), join(target, "COPYRIGHT-library.html"));
  cpSync(join(docs, "licenses"), join(target, "licenses"), { recursive: true });
}

function main() {
  const { values } = parseArgs({
    options: {
      "toolkit-build": { type: "string" },
      "shell-build": { type: "string" },
      upstream: { type: "string" },
      target: { type: "string" },
      out: { type: "string" },
    },
  });
  for (const key of ["toolkit-build", "shell-build", "upstream", "target", "out"]) {
    check(values[key], "--" + key + " is required");
  }
  const upstream = realpathSync(values.upstream);
  const bySource = new Map([
    ["toolkit", toolkit],
    ["oh-my-pi", upstream],
  ]);
  const artifacts = [
    ...pathsFromBuild(values["toolkit-build"], ["omp-refactor", "omp-toolkit-mcp"]),
    ...pathsFromBuild(values["shell-build"], ["omp_shell_cli", "obash"]),
  ];
  const manifests = new Map(artifacts.map((artifact) => [artifact.manifest_path, artifact]));
  const output = resolve(values.out);
  const packages = [...manifests.values()].map((artifact) => capturePackage(artifact, bySource))
    .sort((a, b) => (a.label + a.category).localeCompare(b.label + b.category, "en"));
  const keys = new Set(packages.map(({ category, label }) => category + "/" + label));
  check(keys.size === packages.length, "Multiple sources share the same Cargo package identity");
  copyRustRuntime(output);
  const rows = packages.map((item) => writePackage(item, output));
  const document = [
    "# Binary distribution notices", "", "Rust target: " + values.target, "",
    "This index uses the successful locked Cargo builds for the Rust executables in this archive.",
    "It conservatively includes build dependencies and proc macros reported by Cargo.", "",
    "The [Rust Standard Library copyright notice](rust-standard-library/COPYRIGHT-library.html)",
    "and applicable toolchain license texts are included alongside the dependency notices.", "",
    "Where multiple licenses are offered, the metadata preserves the original declaration and notes any selected license.",
    "The Source Code Form for MPL-covered crates is shipped in the corresponding notice directory.", "",
    "| Package | Source group | Declared license | Provenance | Included notices |",
    "| --- | --- | --- | --- | --- |", ...rows, "",
  ].join("\n");
  writeFileSync(join(output, "BINARY_DEPENDENCIES.md"), document);
  console.log("Recorded " + packages.length + " Cargo packages for " + values.target);
}

try {
  main();
} catch (error) {
  console.error("binary notices: " + error.message);
  process.exitCode = 1;
}
