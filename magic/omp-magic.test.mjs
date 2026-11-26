import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { blockHints, compileLineEdits } from './magic-lines.ts';
import { splitMagicSections } from './magic-sections.ts';
import { fileViews } from './magic-view.ts';

test('file headers inside fenced examples stay in their section', () => {
  const { runs, outside } = splitMagicSections('# a.ts\n```md\n# example\n```\n# b.ts\nRename this.');
  assert.equal(outside, '');
  assert.deepEqual(runs.map(run => run.path), ['a.ts', 'b.ts']);
  assert.match(runs[0].text, /# example/);
});

test('upstream operations compile against original line numbers', () => {
  const source = 'let a = 1;\nlet b = 2;\nlet c = 3;\n';
  const [view] = fileViews(source, 'change a and c');
  const result = compileLineEdits('```json\n[["sub","1","1","4"],["remove",3]]\n```', source, view);
  assert.ok('edit' in result);
  assert.equal(source.replace(result.edit.old_string, () => result.edit.new_string), 'let a = 4;\nlet b = 2;\n');
  assert.match(compileLineEdits('["delete",1,2]\n["replace",2,2,"x"]', source, view).problem, /touch the same lines/);
});

test('substitutions treat dollar markers literally and bound expansion', () => {
  const source = 'before\nother\n';
  const [view] = fileViews(source, 'change before');
  for (const line of [1, 2, 'all']) {
    const text = "after$& $$ $'";
    const result = compileLineEdits(JSON.stringify(['sub', line, 'before', text]), source, view);
    assert.ok('edit' in result);
    assert.equal(source.replace(result.edit.old_string, () => result.edit.new_string), `${text}\nother\n`);
  }
  const large = 'a'.repeat(200_000);
  const [largeView] = fileViews(large, 'expand');
  assert.match(compileLineEdits('["sub","all","a","abcdefghij"]', large, largeView).problem, /too much text/);
  assert.match(compileLineEdits('["constructor",1]', source, view).problem, /unknown operation/);
});

test('multiline substitutions inherit indentation shifts', () => {
  const source = 'function run() {\n  oldCall();\n}\n';
  const [view] = fileViews(source, 'split and indent the call');
  const operations = [['sub', 2, 'oldCall();', 'first();\n  second("$&");'], ['indent', 2, 2, 2]];
  const result = compileLineEdits(JSON.stringify(operations), source, view);
  assert.ok('edit' in result);
  assert.equal(source.replace(result.edit.old_string, () => result.edit.new_string),
    'function run() {\n    first();\n    second("$&");\n}\n');
});

test('excerpts retain absolute line numbers and reject edits in omitted ranges', () => {
  const source = Array.from({ length: 300 }, (_, index) => `const step${index + 1} = ${index + 1};\n`).join('');
  const [excerpt, whole] = fileViews(source, 'Replace the initializer on line 120.');
  assert.equal(excerpt.partial, true);
  assert.equal(whole.partial, false);
  assert.match(excerpt.text, /120\tconst step120/);
  assert.match(excerpt.text, /⋮ lines/);
  assert.match(compileLineEdits('["delete",200,200]', source, excerpt).problem, /not shown/);
  assert.ok('edit' in compileLineEdits('["delete",200,200]', source, whole));
});

test('block hints flag operations that run beyond a block', () => {
  const source = 'start();\nif (ready) {\n  first();\n}\nend();\n';
  const [view] = fileViews(source, 'delete block and end call');
  const result = compileLineEdits('["delete",2,5]', source, view);
  assert.ok('edit' in result);
  assert.match(blockHints(source, result.ranges, view).join('\n'), /line 2 opens a block that ends on line 4/);
});

test('CLI preserves the working tree and checks snapshot, paths, and syntax', {
  skip: process.env.OMP_REFACTOR_BIN ? false : 'Set OMP_REFACTOR_BIN to the built native bridge',
}, () => {
  const root = mkdtempSync(join(tmpdir(), 'omp-magic-'));
  try {
    const cli = fileURLToPath(new URL('./omp-magic.mjs', import.meta.url));
    const source = '\uFEFFconst item = oldCall(1);\r\n';
    const path = '-sample.ts';
    const sample = join(root, path);
    const plan = join(root, 'plan.json');
    const launched = (command, tail, input, status = 0) => {
      const result = spawnSync(process.execPath, [cli, command, '--root', root, ...tail], { input, encoding: 'utf8' });
      assert.equal(result.status, status, result.error?.message || result.stderr);
      if (status !== 0) assert.equal(result.stdout, '');
      return status === 0 ? result.stdout : result.stderr;
    };
    writeFileSync(sample, source);
    const prepared = launched('prepare', ['--instructions', '-'], `# ${path}\nRename oldCall.\n# ${path}\nKeep the argument.\n`);
    writeFileSync(plan, prepared);
    const { files, systemPrompt } = JSON.parse(prepared);
    assert.match(systemPrompt, /line operations/);
    assert.equal(files.length, 1);
    assert.equal(files[0].view, 'full');
    assert.match(files[0].request, /1\tconst item = oldCall\(1\)/);
    assert.match(files[0].request, /Rename oldCall\.\n\nKeep the argument\./);
    const preview = (file, operations, status = 0) => launched('preview', ['--plan', plan, '--replies', '-'],
      JSON.stringify({ replies: [{ path: file, operations }] }), status);
    const patch = preview(path, '["sub",1,"oldCall","newCall"]');
    assert.match(patch, /\+\uFEFFconst item = newCall\(1\);\r\n/);
    const git = spawnSync('git', ['-C', root, '-c', 'core.autocrlf=false', 'apply', '--check', '-'], {
      input: patch, encoding: 'utf8',
    });
    assert.equal(git.status, 0, git.stderr);
    assert.equal(readFileSync(sample, 'utf8'), source);
    assert.match(preview('../other.ts', '["delete",1,1]', 1), /exact plan path/);
    const syntaxError = preview(path, '["insert","before",1,"const helper = 2;"]\n["replace",1,1,"const = ;"]', 1);
    assert.match(syntaxError, /syntax error/);
    assert.match(syntaxError, /Operation 2 .* breaks syntax around line 1/);
    assert.doesNotMatch(syntaxError, /Operation 1 .* breaks syntax/);
    writeFileSync(sample, source.replace('(1)', '(2)'));
    assert.match(preview(path, '["sub",1,"oldCall","newCall"]', 1), /Source changed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
