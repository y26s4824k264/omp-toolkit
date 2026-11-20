#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { createInterface } from "node:readline";
import { parseArgs } from "node:util";

const help = `Usage: codex-context-stats [--turns] [--requests] [--turn INDEX_OR_ID] ROLLOUT.jsonl

Report observed Codex request usage as JSON. --turns includes per-turn usage,
--requests includes each measured request, and --turn selects one turn.
Input growth compares recorded request input sizes. A context window is a
model capacity, and cumulative token usage alone cannot identify a request.
`;

const turnStart = new Set(["task_started", "turn_started"]);
const turnEnd = new Set(["task_complete", "turn_complete", "turn_completed", "turn_aborted", "task_aborted"]);
const contextLine = /"type"\s*:\s*"turn_context"/;
const eventLine = /"type"\s*:\s*"event_msg"/;
const usageEventLine = /"type"\s*:\s*"(?:task_started|turn_started|task_complete|turn_complete|turn_completed|turn_aborted|task_aborted|token_count)"/;

function tokens(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function ratio(numerator, denominator) {
  return numerator !== null && denominator > 0 ? numerator / denominator : null;
}

function cumulativeSnapshot(usage) {
  if (tokens(usage?.input_tokens) === null) return null;
  return JSON.stringify([
    usage.input_tokens, tokens(usage.cached_input_tokens), tokens(usage.cache_write_input_tokens),
    tokens(usage.output_tokens), tokens(usage.reasoning_output_tokens), tokens(usage.total_tokens),
  ]);
}

function summarize(requests) {
  const first = requests[0];
  const last = requests.at(-1);
  let input = 0;
  let cached = 0;
  let cachedCount = 0;
  let peak = 0;
  for (const request of requests) {
    input += request.input_tokens;
    peak = Math.max(peak, request.input_tokens);
    if (request.cached_input_tokens !== null) {
      cached += request.cached_input_tokens;
      cachedCount += 1;
    }
  }
  const fullCacheCoverage = cachedCount > 0 && cachedCount === requests.length;
  return {
    request_count: requests.length,
    first_request_input_tokens: first?.input_tokens ?? null,
    first_request_model: first?.model ?? null,
    first_request_reasoning_effort: first?.reasoning_effort ?? null,
    first_request_cached_input_ratio: first?.cached_input_ratio ?? null,
    first_request_context_window_tokens: first?.context_window_tokens ?? null,
    first_request_input_context_ratio: first?.input_context_ratio ?? null,
    last_request_input_tokens: last?.input_tokens ?? null,
    input_growth_tokens: requests.length > 1 ? last.input_tokens - first.input_tokens : null,
    peak_request_input_tokens: first ? peak : null,
    sum_request_input_tokens: first ? input : null,
    requests_with_cache_counts: cachedCount,
    cached_input_ratio: fullCacheCoverage ? ratio(cached, input) : null,
  };
}

async function readUsage(path) {
  const requests = [];
  const turns = new Map();
  const counts = { duplicate_snapshots: 0, cumulative_only_records: 0, missing_usage_records: 0, invalid_metadata_lines: 0 };
  let activeTurn = null;
  let previousCumulative = null;
  let cumulativeHasRequest = false;
  const lines = createInterface({ input: createReadStream(path), crlfDelay: Infinity });

  function getTurn(id) {
    if (typeof id !== "string" || id.length === 0) return null;
    if (!turns.has(id)) {
      turns.set(id, { index: turns.size + 1, id, model: null, reasoning_effort: null, context_window_tokens: null, requests: [] });
    }
    return turns.get(id);
  }

  for await (const line of lines) {
    if (!contextLine.test(line) && !(eventLine.test(line) && usageEventLine.test(line))) continue;
    let record;
    try {
      record = JSON.parse(line);
    } catch {
      counts.invalid_metadata_lines += 1;
      continue;
    }
    const payload = record?.payload;
    if (record?.type === "turn_context") {
      const turn = getTurn(payload?.turn_id);
      if (turn) {
        if (typeof payload.model === "string") turn.model = payload.model;
        if (typeof payload.effort === "string") turn.reasoning_effort = payload.effort;
        activeTurn ??= turn;
      }
      continue;
    }
    if (record?.type !== "event_msg" || !payload) continue;
    if (turnStart.has(payload.type)) {
      activeTurn = getTurn(payload.turn_id);
      if (activeTurn) activeTurn.context_window_tokens = tokens(payload.model_context_window);
      continue;
    }
    if (turnEnd.has(payload.type)) {
      if (!payload.turn_id || activeTurn?.id === payload.turn_id) activeTurn = null;
      continue;
    }
    if (payload.type !== "token_count") continue;

    const info = payload.info;
    const cumulative = cumulativeSnapshot(info?.total_token_usage);
    const usage = info?.last_token_usage;
    const input = tokens(usage?.input_tokens);
    if (cumulative !== null && cumulative === previousCumulative && (cumulativeHasRequest || input === null)) {
      counts.duplicate_snapshots += 1;
      continue;
    }
    if (cumulative !== null) {
      previousCumulative = cumulative;
      cumulativeHasRequest = input !== null;
    }
    if (input === null) {
      if (cumulative === null) counts.missing_usage_records += 1;
      else counts.cumulative_only_records += 1;
      continue;
    }

    const turn = getTurn(payload.turn_id) ?? activeTurn;
    const recordedCache = tokens(usage.cached_input_tokens);
    const cached = recordedCache !== null && recordedCache <= input ? recordedCache : null;
    const contextWindow = tokens(info.model_context_window) ?? turn?.context_window_tokens ?? null;
    const previous = requests.at(-1);
    const request = {
      index: requests.length + 1,
      turn_index: turn?.index ?? null,
      timestamp: typeof record.timestamp === "string" ? record.timestamp : null,
      model: turn?.model ?? null,
      reasoning_effort: turn?.reasoning_effort ?? null,
      input_tokens: input,
      cached_input_tokens: cached,
      cached_input_ratio: ratio(cached, input),
      context_window_tokens: contextWindow,
      input_context_ratio: ratio(input, contextWindow),
      input_growth_tokens: previous ? input - previous.input_tokens : null,
    };
    requests.push(request);
    turn?.requests.push(request);
  }

  return { requests, turns: [...turns.values()], counts };
}

async function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      help: { type: "boolean", short: "h" },
      turns: { type: "boolean" },
      requests: { type: "boolean" },
      turn: { type: "string" },
    },
  });
  if (values.help) {
    process.stdout.write(help);
    return;
  }
  if (positionals.length !== 1) throw new Error("Expected one Codex rollout path. See --help.");

  const { requests, turns, counts } = await readUsage(positionals[0]);
  const selected = values.turn === undefined ? null : turns.find(
    turn => String(turn.index) === values.turn || turn.id === values.turn,
  );
  if (selected === undefined) throw new Error(`No turn matches ${JSON.stringify(values.turn)}.`);
  const measured = selected?.requests ?? requests;

  const report = {
    ...summarize(measured),
    rollout_turn_count: turns.length,
    rollout_records: counts,
  };

  if (values.turns || selected) {
    let previousFirst = null;
    report.turns = turns.map(({ requests: turnRequests, ...turn }) => {
      const stats = summarize(turnRequests);
      const first = stats.first_request_input_tokens;
      const growth = first !== null && previousFirst !== null ? first - previousFirst : null;
      previousFirst = first;
      return { ...turn, ...stats, first_request_growth_from_previous_turn_tokens: growth };
    });
    if (selected) report.turns = report.turns.filter(turn => turn.index === selected.index);
  }
  if (values.requests) report.requests = measured;
  console.log(JSON.stringify(report, null, 2));
}

main().catch(error => {
  console.error(`codex-context-stats: ${error.message}`);
  process.exitCode = 1;
});
