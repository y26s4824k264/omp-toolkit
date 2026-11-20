import { constants } from "node:fs";
import { open } from "node:fs/promises";
import { isAbsolute } from "node:path";
import { stripVTControlCharacters } from "node:util";

import { isCpuProfilePath, renderCpuProfile } from "./cpuprofile.ts";
import { isSampleProfilePath, renderSampleProfile } from "./sample-profile.ts";

const MAX_INPUT_BYTES = 32 * 1024 * 1024;
const MAX_OUTPUT_BYTES = 12_000;
const OMISSION = "\n[Middle of profile summary omitted]\n";

async function readProfile(path) {
  const flags = constants.O_RDONLY | constants.O_NONBLOCK | constants.O_NOFOLLOW;
  const file = await open(path, flags);
  try {
    const stat = await file.stat();
    if (!stat.isFile()) throw new Error("Expected an existing regular file.");
    if (stat.size > MAX_INPUT_BYTES) throw new Error("Profile exceeds the 32 MiB input limit.");

    const chunks = [];
    let size = 0;
    for await (const chunk of file.createReadStream({ start: 0, end: MAX_INPUT_BYTES, autoClose: false })) {
      size += chunk.length;
      if (size > MAX_INPUT_BYTES) throw new Error("Profile exceeds the 32 MiB input limit.");
      chunks.push(chunk);
    }
    return new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks, size));
  } finally {
    await file.close();
  }
}

function reportPreview(summary) {
  const clean = stripVTControlCharacters(summary)
    .replace(/Use ':raw' or a line range to read the original (JSON|file)\./g, "Read the source report to inspect individual frames.")
    .replace(/[\u0000-\u0008\u000B-\u001F\u007F-\u009F]/gu, "")
    .trimEnd();
  const bytes = Buffer.from(clean + "\n");
  if (bytes.length <= MAX_OUTPUT_BYTES) return bytes;

  const budget = MAX_OUTPUT_BYTES - Buffer.byteLength(OMISSION);
  let head = Math.floor(budget / 2);
  let tail = bytes.length - (budget - head);
  const headLine = bytes.lastIndexOf(0x0a, head - 1);
  const tailLine = bytes.indexOf(0x0a, tail);
  if (headLine > 0) head = headLine;
  if (tailLine >= 0 && tailLine < bytes.length - 1) tail = tailLine + 1;
  while (head > 0 && (bytes[head] & 0xc0) === 0x80) head--;
  while (tail < bytes.length && (bytes[tail] & 0xc0) === 0x80) tail++;

  return Buffer.concat([bytes.subarray(0, head), Buffer.from(OMISSION), bytes.subarray(tail)]);
}

async function main() {
  const [path, ...rest] = process.argv.slice(2);
  if (!path || rest.length || !isAbsolute(path)) {
    throw new Error("Usage: node summary.mjs /absolute/path/to/report.cpuprofile|report.sample.txt");
  }

  const isCpu = isCpuProfilePath(path);
  if (!isCpu && !isSampleProfilePath(path)) {
    throw new Error("Unsupported profile extension. Expected .cpuprofile or .sample.txt.");
  }

  const profile = await readProfile(path);
  const name = isCpu ? "V8 CPU profile" : "macOS sample report";
  let summary;
  try {
    summary = isCpu ? renderCpuProfile(profile) : renderSampleProfile(profile.replaceAll("\r\n", "\n"));
  } catch (error) {
    throw new Error(error instanceof RangeError ? name + " call tree is too deep." : "Invalid " + name + ".");
  }
  if (!summary) throw new Error("Invalid or empty " + name + ".");
  process.stdout.write(reportPreview(summary));
}

await main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
