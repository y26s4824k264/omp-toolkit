import { Settings } from "@oh-my-pi/pi-coding-agent";
import { LspTool } from "@oh-my-pi/pi-coding-agent/lsp";
import { shutdownAll } from "@oh-my-pi/pi-coding-agent/lsp/client";
import { getConfig } from "@oh-my-pi/pi-coding-agent/lsp/config";

function catalogue() {
  const config = getConfig(process.env.PI_CODING_AGENT_DIR);
  const servers = { ...config.nested?.servers, ...config.unresolved, ...config.servers };
  return Object.entries(servers).sort(([a], [b]) => a.localeCompare(b)).map(([name, server]) => ({
    name, command: server.command, file_types: server.fileTypes, root_markers: server.rootMarkers,
  }));
}

function outcome(action, details, text) {
  if (details?.success !== true) return "failed";
  if (action !== "diagnostics") return "completed";
  if (!details.serverName || !text) return "incomplete";
  if (/no diagnostics report yet from |some servers failed \(/.test(text)) return "incomplete";
  // OMP v19 returns formatted diagnostics and a report-success flag, not severity totals.
  const summary = text.split("\n", 1)[0];
  if (/^[1-9]\d* error\(s\)(?:,|:|$)/.test(summary)) return "errors";
  return "completed";
}

export default async function run() {
  const requests = JSON.parse(process.env.OMP_CODEX_LSP_REQUESTS);
  let result;
  const results = [];
  try {
    if (requests[0].action === "catalog") {
      result = { exit_code: 0, servers: catalogue() };
    } else {
      const tool = new LspTool({ cwd: process.cwd(), settings: Settings.isolated(), lspReadOnly: true });
      for (const [index, request] of requests.entries()) {
        try {
          if (request.action === "rename-preview" || request.action === "rename-file-preview") {
            const { renamePreview } = await import("./rename-preview.mjs");
            results.push(await renamePreview(process.cwd(), process.env.PI_CODING_AGENT_DIR, request));
            continue;
          }
          const reply = await tool.execute(`codex-lsp-${index}`, request);
          const text = reply.content.filter(item => item.type === "text").map(item => item.text).join("\n");
          results.push({ file: request.file, server: reply.details?.serverName, status: outcome(request.action, reply.details, text), text });
        } catch (error) {
          results.push({ file: request.file, status: "failed", text: error.message });
        }
      }
      const failed = results.some(entry => entry.status === "failed" || entry.status === "incomplete");
      const errors = results.some(entry => entry.status === "errors");
      result = { exit_code: failed ? 2 : errors ? 1 : 0, results };
    }
  } finally {
    await shutdownAll();
  }
  console.log(`OMP_CODEX_LSP_RESULT=${JSON.stringify(result)}`);
}
