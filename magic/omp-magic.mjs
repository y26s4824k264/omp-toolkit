import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, existsSync, openSync, readFileSync, readSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { blockHints, compileLineEdits, diagnoseSyntaxFailure } from './magic-lines.ts';
import { splitMagicSections } from './magic-sections.ts';
import { fileViews } from './magic-view.ts';

const maxFiles = 16;
const maxInstructions = 64 * 1024;
const maxView = 384 * 1024;
const maxOperations = 256 * 1024;
const maxJson = 16 * 1024 * 1024;
const runtime = name => new URL(name, import.meta.url);
const systemPrompt = readFileSync(runtime('magic-apply-system.md'), 'utf8').trim();
const requestTemplate = readFileSync(runtime('magic-apply.md'), 'utf8').trim();
const usage = `Usage:
  omp-magic prepare --root PROJECT --instructions FILE [--full]
  omp-magic preview --root PROJECT --plan FILE --replies FILE

Instructions use '# relative/path' followed by the desired changes. Give the plan's
systemPrompt and each file's request to your AI. Save its answers as JSON:
  {"replies":[{"path":"relative/path","operations":"[\\\"delete\\\",12,14]"}]}
Preview validates the operations against the prepared snapshot and prints a Git patch.
If preview asks for block-range confirmation, review the same operation and set
confirmRange to true for that reply. Use --full to prepare without an excerpt.
Pass - to read one input from stdin.`;

function input(file, limit) {
  if (file !== '-' && !statSync(file).isFile()) throw new Error(`${file} must be a regular file`);
  const fd = file === '-' ? 0 : openSync(file, 'r');
  try {
    const buffer = Buffer.allocUnsafe(limit + 1);
    let length = 0;
    while (length < buffer.length) {
      const bytes = readSync(fd, buffer, length, buffer.length - length, null);
      if (bytes === 0) break;
      length += bytes;
    }
    if (length > limit) throw new Error(`${file} exceeds ${limit} bytes`);
    return new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(0, length));
  } finally {
    if (fd !== 0) closeSync(fd);
  }
}

function jsonInput(file) {
  try {
    return JSON.parse(input(file, maxJson));
  } catch (error) {
    throw new Error(`Cannot read JSON from ${file}: ${error.message}`);
  }
}

function native(root, command, args, stdin, timeout = 60_000) {
  const name = process.platform === 'win32' ? 'omp-refactor.exe' : 'omp-refactor';
  const installed = [`../../bin/${name}`, `../../../bin/${name}`]
    .map(path => fileURLToPath(runtime(path))).find(existsSync);
  const binary = process.env.OMP_REFACTOR_BIN || installed || name;
  const result = spawnSync(binary, ['--root', root, command, ...args], {
    input: stdin, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, timeout, windowsHide: true,
  });
  if (result.error) throw new Error(`Cannot run omp-refactor: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || `omp-refactor failed (${result.signal || result.status})`);
  }
  return result.stdout;
}

function snapshots(root, paths) {
  // End native option parsing so a project-relative filename may begin with '-'.
  const values = JSON.parse(native(root, 'edit-snapshot', ['--', ...paths]));
  if (!Array.isArray(values) || values.length !== paths.length) throw new Error('Unexpected snapshot response');
  return values.map(({ path, before }, index) => {
    if (path !== paths[index] || typeof before !== 'string') throw new Error('Unexpected snapshot response');
    return before;
  });
}

const digest = before => createHash('sha256').update(before).digest('hex');
const normalized = before => before.replace(/^\uFEFF/, '').replace(/\r\n/g, '\n');

function sections(text) {
  const { runs, outside } = splitMagicSections(text);
  if (runs.length === 0) throw new Error('Give one # path line followed by its changes per file.');
  if (outside.trim()) throw new Error('Put every change under its file’s # path line.');
  const changes = new Map();
  for (const { path, text } of runs) {
    const previous = changes.get(path) ?? [];
    previous.push(text.trim());
    changes.set(path, previous);
  }
  if (changes.size > maxFiles) throw new Error(`Magic takes at most ${maxFiles} files per batch.`);
  return [...changes].map(([path, parts]) => {
    const change = parts.filter(Boolean).join('\n\n');
    if (!change) throw new Error(`No change is described for ${path}.`);
    return { path, change };
  });
}

function modelRequest(path, change, view) {
  const fields = { path, change, file: view.text };
  return requestTemplate
    .replace(/{{#if partial}}([\s\S]*?){{\/if}}/, (_, text) => view.partial ? text : '')
    .replace(/{{#if previous}}[\s\S]*?{{\/if}}/, '')
    .replace(/{{(path|change|file)}}/g, (_, key) => fields[key])
    .trim();
}

function viewType(view) {
  return view.partial ? 'excerpt' : 'full';
}

function prepare(root, file, full) {
  const entries = sections(input(file, maxInstructions));
  const source = snapshots(root, entries.map(({ path }) => path));
  const files = entries.map(({ path, change }, index) => {
    const before = source[index];
    const content = normalized(before);
    if (!content) throw new Error(`[${path}] Magic edits existing nonempty text.`);
    const view = fileViews(content, change, maxView).find(candidate => !full || !candidate.partial);
    if (!view) {
      const next = full ? 'Drop --full and name the lines to change.' : 'Quote the lines to change, or give their line numbers.';
      throw new Error(`[${path}] No source view fits the ${maxView}-character limit. ${next}`);
    }
    return { path, change, sha256: digest(before), view: viewType(view), request: modelRequest(path, change, view) };
  });
  const plan = JSON.stringify({ version: 1, systemPrompt, files }, null, 2) + '\n';
  if (Buffer.byteLength(plan) > maxJson) throw new Error('The prepared plan is too large. Split the batch.');
  return plan;
}

function checkPlan(plan) {
  if (plan?.version !== 1 || !Array.isArray(plan.files) || !plan.files.length || plan.files.length > maxFiles) {
    throw new Error('Expected a plan produced by omp-magic prepare.');
  }
  const paths = new Set();
  for (const entry of plan.files) {
    if (typeof entry?.path !== 'string' || !entry.path || paths.has(entry.path) ||
        typeof entry.change !== 'string' || !entry.change || Buffer.byteLength(entry.change) > maxInstructions ||
        typeof entry.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(entry.sha256) ||
        !['excerpt', 'full'].includes(entry.view)) {
      throw new Error('The plan contains an invalid or duplicate file entry.');
    }
    paths.add(entry.path);
  }
  return paths;
}

function checkReplies(input, paths) {
  if (!Array.isArray(input?.replies) || input.replies.length !== paths.size) {
    throw new Error('Give one reply per planned file in a top-level replies array.');
  }
  const replies = new Map();
  for (const reply of input.replies) {
    if (!paths.has(reply?.path) || replies.has(reply.path) ||
        typeof reply.operations !== 'string' || Buffer.byteLength(reply.operations) > maxOperations ||
        (reply.confirmRange !== undefined && typeof reply.confirmRange !== 'boolean')) {
      throw new Error('Replies must name each exact plan path once and give bounded operations text.');
    }
    replies.set(reply.path, reply);
  }
  return replies;
}

function preview(root, planFile, repliesFile) {
  const plan = jsonInput(planFile);
  const paths = checkPlan(plan);
  const replies = checkReplies(jsonInput(repliesFile), paths);
  const source = snapshots(root, [...paths]);
  const edits = plan.files.map(({ path, change, sha256, view: selection }, index) => {
    const before = source[index];
    if (digest(before) !== sha256) throw new Error(`[${path}] Source changed. Prepare a new plan.`);
    const content = normalized(before);
    const view = fileViews(content, change, maxView).find(candidate => viewType(candidate) === selection);
    if (!view) throw new Error(`[${path}] The prepared view is unavailable. Prepare a new plan.`);
    const retry = view.partial ? ' Prepare with --full if more context is needed.' : '';
    const reply = replies.get(path);
    const operations = normalized(reply.operations.trim());
    const refusal = operations.match(/^CANNOT:\s*(.*)/i);
    if (refusal) throw new Error(`[${path}] AI declined: ${refusal[1] || 'no reason given'}.${retry}`);
    const compiled = compileLineEdits(operations, content, view);
    if ('problem' in compiled) throw new Error(`[${path}] ${compiled.problem}${retry}`);
    const hints = blockHints(content, compiled.ranges, view);
    if (hints.length && !reply.confirmRange) {
      throw new Error(`[${path}] ${hints.join('\n')}. Correct the range or set confirmRange to true after reviewing it.${retry}`);
    }
    const { old_string, new_string } = compiled.edit;
    const at = content.indexOf(old_string);
    if (at < 0 || at !== content.lastIndexOf(old_string)) throw new Error(`[${path}] Replacement anchor is not unique.`);
    const after = content.slice(0, at) + new_string + content.slice(at + old_string.length);
    return { path, before, after, content, view, operations: compiled.operations };
  });
  const replacements = edits.map(({ path, before, after }) => ({ path, before, after }));
  try {
    return native(root, 'edit-snapshot-preview', [], JSON.stringify(replacements));
  } catch (error) {
    const failed = edits.find(({ path }) => error.message === `Error: Magic edit produces a syntax error in ${path}`);
    if (!failed) throw error;
    const deadline = performance.now() + 2_000;
    let checks = 0;
    let detail;
    try {
      detail = diagnoseSyntaxFailure(failed.operations, failed.content, failed.view, code => {
        const remaining = Math.floor(deadline - performance.now());
        if (++checks > 64 || remaining < 1) throw new Error('Magic diagnostic budget exhausted');
        return native(root, 'edit-snapshot-parse', ['--', failed.path], code, remaining).trim() === 'true';
      });
    } catch {
      // The original validation error is authoritative if diagnosis cannot finish.
      throw error;
    }
    throw new Error(detail ? `${error.message}\n${detail}` : error.message);
  }
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === '--help' || command === '-h' || args.includes('--help') || args.includes('-h')) return usage + '\n';
  const shared = { root: { type: 'string', default: '.' } };
  if (command === 'prepare') {
    const { values } = parseArgs({ args, options: { ...shared, instructions: { type: 'string' }, full: { type: 'boolean' } } });
    if (!values.instructions) throw new Error('prepare requires --instructions FILE');
    return prepare(values.root, values.instructions, values.full);
  }
  if (command === 'preview') {
    const { values } = parseArgs({ args, options: { ...shared, plan: { type: 'string' }, replies: { type: 'string' } } });
    if (!values.plan || !values.replies) throw new Error('preview requires --plan FILE and --replies FILE');
    if (values.plan === '-' && values.replies === '-') throw new Error('Only one input can use stdin.');
    return preview(values.root, values.plan, values.replies);
  }
  throw new Error(usage);
}

try {
  process.stdout.write(main());
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
