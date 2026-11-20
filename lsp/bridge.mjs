import { resolve } from "node:path";
import { LspClient, symbolPosition } from "./client.mjs";
import { renamePreview } from "./rename-preview.mjs";
import { catalogue, selectServers, serverDefinitions, serverStatus, workspaceDocument } from "./servers.mjs";

const textMethods = {
  definition: "textDocument/definition",
  references: "textDocument/references",
  hover: "textDocument/hover",
  type_definition: "textDocument/typeDefinition",
  implementation: "textDocument/implementation",
  symbols: "textDocument/documentSymbol",
};

function formatDiagnostics(items) {
  const severity = ["", "error", "warning", "information", "hint"];
  if (!items.length) return "No diagnostics";
  const errors = items.filter(item => item.severity === 1).length;
  const lines = items.map(item => {
    const start = item.range?.start;
    const position = start ? (start.line + 1) + ":" + (start.character + 1) + ": " : "";
    const code = item.code === undefined ? "" : " [" + item.code + "]";
    return position + (severity[item.severity] || "diagnostic") + ": " + item.message + code;
  });
  return [errors + " error(s), " + (items.length - errors) + " other diagnostic(s)", ...lines].join("\n");
}

async function diagnose(client, server, file, signal) {
  const items = await client.diagnostics(file, signal);
  return { server: server.name, errors: items.some(item => item.severity === 1), text: formatDiagnostics(items) };
}

async function queryDocument(client, file, request, signal) {
  if (request.action === "capabilities") return client.capabilities;
  const document = client.open(file);
  const params = { textDocument: { uri: document.uri } };
  if (request.action !== "symbols") params.position = symbolPosition(document.text, request.symbol, request.line);
  if (request.action === "references") params.context = { includeDeclaration: true };
  if (request.action === "references" || request.action === "definition") await client.waitForIndex(signal, file);
  return await client.request(textMethods[request.action], params, signal);
}

async function queryWorkspace(candidates, request, signal, connect) {
  const symbols = [];
  const servers = [];
  const failures = [];
  for (const server of candidates) {
    try {
      const client = await connect(server, signal);
      if (!client.capabilities.workspaceSymbolProvider && !client.hasCapability("workspace/symbol")) continue;
      const seed = server.name === "typescript-language-server" ? workspaceDocument(server) : undefined;
      await client.waitForIndex(signal, seed);
      const reply = await client.request("workspace/symbol", { query: request.query.trim() }, signal);
      if (Array.isArray(reply)) symbols.push(...reply);
      servers.push(server.name);
    } catch (error) {
      failures.push(server.name + ": " + error.message);
    }
  }
  return {
    file: request.file, server: servers.join(", "), status: servers.length && !failures.length ? "completed" : "incomplete",
    text: servers.length ? JSON.stringify(symbols, null, 2)
      : failures.join("\n") || "No language server supports workspace symbols",
    ...(servers.length && failures.length ? { failures } : {}),
  };
}

export async function runRequests(root, requests, scratch) {
  if (requests[0].action === "catalog") return { exit_code: 0, servers: catalogue() };
  const definitions = serverDefinitions(root);
  if (requests[0].action === "status") {
    const servers = serverStatus(definitions, root);
    return {
      exit_code: 0,
      results: [{ status: "completed", servers, text: servers.map(server =>
        server.name + ": " + (server.available ? "available" : "unavailable") + " (" + server.command + ")"
      ).join("\n") }],
    };
  }
  const clients = new Map();
  async function connect(server, signal) {
    const key = server.name + "\0" + server.root;
    if (!clients.has(key)) {
      const client = new LspClient(server, scratch);
      try {
        await client.initialize(signal);
        clients.set(key, client);
      } catch (error) {
        await client.close();
        throw error;
      }
    }
    return clients.get(key);
  }

  const results = [];
  try {
    for (const request of requests) {
      const { action } = request;
      const workspace = action === "symbols" && request.file === "*";
      const file = workspace ? undefined : resolve(root, request.file);
      const signal = AbortSignal.timeout(request.timeout * 1000);
      if (action === "rename-preview" || action === "rename-file-preview") {
        try {
          results.push(await renamePreview(definitions, root, scratch, request, signal, connect));
        } catch (error) {
          results.push({ file: request.file, status: "failed", text: error.message });
        }
        continue;
      }
      const candidates = selectServers(definitions, root, file, action);
      if (!candidates.length) {
        results.push({ file: request.file, status: "incomplete", text: "No language server handles " + (request.file === "*" ? "this workspace" : request.file) });
        continue;
      }
      if (workspace) {
        results.push(await queryWorkspace(candidates, request, signal, connect));
        continue;
      }
      if (action === "diagnostics") {
        const primary = candidates.find(server => !server.isLinter);
        const selected = [primary, ...candidates.filter(server => server.isLinter)].filter(Boolean);
        const reports = [];
        const failed = [];
        for (const server of selected) {
          try {
            reports.push(await diagnose(await connect(server, signal), server, file, signal));
          } catch (error) {
            failed.push(server.name + ": " + error.message);
          }
        }
        const multiple = reports.length > 1;
        const text = reports.map(report => (multiple ? report.server + ":\n" : "") + report.text);
        if (failed.length) text.push("No complete diagnostics report:\n" + failed.join("\n"));
        results.push({ file: request.file, server: selected.map(server => server.name).join(", "),
          status: failed.length ? "incomplete" : reports.some(report => report.errors) ? "errors" : "completed",
          text: text.join("\n") });
        continue;
      }
      let report;
      const failures = [];
      for (const server of candidates) {
        try {
          const client = await connect(server, signal);
          const value = await queryDocument(client, file, request, signal);
          report = {
            file: request.file, server: server.name, status: "completed",
            text: JSON.stringify(value ?? null, null, 2),
          };
          break;
        } catch (error) {
          failures.push(server.name + ": " + error.message);
        }
      }
      results.push(report ?? { file: request.file, status: "incomplete",
        text: failures.join("\n") || "No language server supports this request" });
    }
  } finally {
    await Promise.all([...clients.values()].map(client => client.close()));
  }
  const failed = results.some(entry => entry.status === "failed" || entry.status === "incomplete");
  return { exit_code: failed ? 2 : results.some(entry => entry.status === "errors") ? 1 : 0, results };
}
