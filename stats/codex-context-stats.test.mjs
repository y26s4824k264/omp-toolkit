import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmdirSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("codex-context-stats.mjs", import.meta.url));
const event = (type, fields = {}) => ({ payload: { type, ...fields }, type: "event_msg" });
const usage = (input, cached) => ({ input_tokens: input, cached_input_tokens: cached });
const sample = (last, total, window) => event("token_count", {
  info: { last_token_usage: last, total_token_usage: total, model_context_window: window },
});

function fixture(t, records) {
  const dir = mkdtempSync(join(tmpdir(), "codex-context-stats-"));
  const path = join(dir, "rollout.jsonl");
  const lines = records.map(record => typeof record === "string" ? record : JSON.stringify(record));
  writeFileSync(path, `${lines.join("\n")}\n`);
  t.after(() => {
    unlinkSync(path);
    rmdirSync(dir);
  });
  return (...args) => {
    const processResult = spawnSync(process.execPath, [cli, path, ...args], { encoding: "utf8" });
    assert.equal(processResult.status, 0, processResult.stderr);
    assert.doesNotMatch(processResult.stdout, /private transcript/);
    return JSON.parse(processResult.stdout);
  };
}

test("groups fresh usage by turn and excludes unchanged cumulative snapshots", t => {
  const read = fixture(t, [
    event("task_started", { turn_id: "one", model_context_window: 1000 }),
    { type: "turn_context", payload: { turn_id: "one", model: "test-model", effort: "high" } },
    event("user_message", { message: "private transcript" }),
    sample(usage(100, 0), usage(100, 0)),
    sample(usage(100, 0), usage(100, 0)),
    sample(usage(140, 100), usage(240, 100), 800),
    event("task_complete", { turn_id: "one" }),
    event("task_started", { turn_id: "two", model_context_window: 1000 }),
    sample(null, usage(320, 160)),
    sample(usage(80, 60), usage(320, 160)),
    event("task_complete", { turn_id: "two" }),
    sample(usage(90, 70), usage(410, 230)),
  ]);
  const report = read("--turns", "--requests");
  assert.equal(report.first_request_input_tokens, 100);
  assert.equal(report.first_request_model, "test-model");
  assert.equal(report.first_request_reasoning_effort, "high");
  assert.equal(report.first_request_input_context_ratio, 0.1);
  assert.equal(report.sum_request_input_tokens, 410);
  assert.equal(report.cached_input_ratio, 230 / 410);
  assert.deepEqual(report.rollout_records, {
    duplicate_snapshots: 1, cumulative_only_records: 1, missing_usage_records: 0, invalid_metadata_lines: 0,
  });
  assert.deepEqual(report.turns.map(turn => turn.request_count), [2, 1]);
  assert.equal(report.turns[1].first_request_growth_from_previous_turn_tokens, -20);
  assert.equal(report.requests[1].context_window_tokens, 800);
  assert.deepEqual(report.requests.map(request => request.input_growth_tokens), [null, 40, -60, 10]);
  assert.deepEqual(report.requests.map(request => request.turn_index), [1, 1, 2, null]);
  const selected = read("--turn", "two", "--requests");
  assert.equal(selected.first_request_input_tokens, 80);
  assert.equal(selected.cached_input_ratio, 0.75);
  assert.deepEqual(selected.turns.map(turn => turn.index), [2]);
  assert.deepEqual(read("--turn", "2").turns, selected.turns);
});

test("labels missing per-request usage and cache data without using input capacity or totals", t => {
  const read = fixture(t, [
    event("task_started", { turn_id: "one", model_context_window: 2000 }),
    event("token_count", { info: null }),
    sample(null, usage(350, 110)),
    event("task_complete", { turn_id: "one" }),
    event("task_started", { turn_id: "two" }),
    sample(usage(80), null),
    sample(usage(80, 100), null),
    '{"type":"event_msg","payload":{"type":"token_count"',
  ]);
  const absent = read("--turn", "1");
  assert.equal(absent.request_count, 0);
  assert.equal(absent.first_request_input_tokens, null);
  assert.equal(absent.cached_input_ratio, null);
  assert.equal(absent.turns[0].context_window_tokens, 2000);
  const present = read("--requests");
  assert.equal(present.request_count, 2);
  assert.equal(present.first_request_input_tokens, 80);
  assert.equal(present.cached_input_ratio, null);
  assert.equal(present.first_request_context_window_tokens, null);
  assert.equal(present.requests[1].cached_input_tokens, null);
  assert.deepEqual(present.rollout_records, {
    duplicate_snapshots: 0, cumulative_only_records: 1, missing_usage_records: 1, invalid_metadata_lines: 1,
  });
});
