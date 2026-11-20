import { fileURLToPath } from "node:url";
import {
  ensureWorkspaceFolders, getOrCreateClient, isRustAnalyzerClient, reconcileFileFromDisk,
  reconcileOpenFilesFromDisk, sendRequest, supportsMethod, unservedCheckout, waitForProjectLoaded,
} from "@oh-my-pi/pi-coding-agent/lsp/client";
import { getConfig } from "@oh-my-pi/pi-coding-agent/lsp/config";
import { hasRustWorkspaceAncestor } from "@oh-my-pi/pi-coding-agent/lsp/diagnostics";
import { applyTextEditsToString, flattenWorkspaceTextEdits } from "@oh-my-pi/pi-coding-agent/lsp/edits";
import { getLspServersForFile, isMethodNotFoundError, isProjectAwareLspServer } from "@oh-my-pi/pi-coding-agent/lsp/servers";
import { fileToUri, findSymbolInFile, resolveSymbolColumn } from "@oh-my-pi/pi-coding-agent/lsp/utils";
import {
  checkRenameDestination, encodeReplacement, readRenameSnapshot, renameApplyRoot, writeRenamePatch,
} from "./rename-patch.mjs";

const symbolMethod = "textDocument/rename";
const moveMethod = "workspace/willRenameFiles";

async function selectSymbolServer(candidates, root, signal) {
  const failures = [];
  let fallback;
  for (const [name, config] of candidates) {
    signal.throwIfAborted();
    try {
      const client = await getOrCreateClient(config, root, undefined, signal);
      const entry = { name, config, client };
      if (supportsMethod(client, "renameProvider", symbolMethod)) return entry;
      fallback ??= entry;
    } catch (error) {
      failures.push(`${name}: ${error.message}`);
    }
  }
  signal.throwIfAborted();
  if (fallback) return fallback;
  throw new Error(failures.length ? `No rename language server started (${failures.join(", ")})` : "No language server handles the source file");
}

async function requireCompleteIndex(entry, files, signal) {
  signal.throwIfAborted();
  if (entry.client.activeProgressTokens.size > 0) {
    throw new Error(`${entry.name} is still indexing; retry the rename preview`);
  }
  for (const file of files) {
    if (await unservedCheckout(entry.client, file)) {
      throw new Error(`${entry.name} did not index the checkout containing ${file}`);
    }
  }
}

async function symbolEdit(config, root, source, request, signal) {
  const candidates = getLspServersForFile(config, source.path);
  const entry = await selectSymbolServer(candidates, root, signal);
  await reconcileFileFromDisk(entry.client, source.path, signal);
  const rust = entry.name === "rust-analyzer" || isRustAnalyzerClient(entry.client);
  if (isProjectAwareLspServer(entry.config) && (!rust || hasRustWorkspaceAncestor(source.path))) {
    await waitForProjectLoaded(entry.client, signal);
  }
  const location = request.line === undefined
    ? await findSymbolInFile(source.path, request.symbol)
    : { line: request.line, character: await resolveSymbolColumn(source.path, request.line, request.symbol) };
  await reconcileOpenFilesFromDisk(entry.client, signal, undefined, "throw");
  const edit = await sendRequest(entry.client, symbolMethod, {
    textDocument: { uri: fileToUri(source.path) },
    position: { line: location.line - 1, character: location.character },
    newName: request.new_name,
  }, signal);
  await requireCompleteIndex(entry, [source.path], signal);
  return [{ ...entry, edit }];
}

async function fileEdits(config, root, source, target, signal) {
  const sourceServers = new Map(getLspServersForFile(config, source.path));
  const servers = new Map([...sourceServers, ...getLspServersForFile(config, target.path)]);
  const files = [{ oldUri: fileToUri(source.path), newUri: fileToUri(target.path) }];
  const replies = [];
  for (const [name, server] of servers) {
    signal.throwIfAborted();
    const client = await getOrCreateClient(server, root, undefined, signal);
    // Without this capability the server never owes a file-rename response.
    const advertised = client.serverCapabilities?.workspace?.fileOperations?.willRename;
    const registered = Array.from(client.dynamicCapabilityRegistrations?.values() ?? []).includes(moveMethod);
    if (!advertised && !registered) continue;
    await ensureWorkspaceFolders(client, [source.path, target.path], signal);
    if (sourceServers.has(name)) await reconcileFileFromDisk(client, source.path, signal);
    if (isProjectAwareLspServer(server)) await waitForProjectLoaded(client, signal);
    await reconcileOpenFilesFromDisk(client, signal, undefined, "throw");
    let edit;
    try {
      edit = await sendRequest(client, moveMethod, { files }, signal);
    } catch (error) {
      if (isMethodNotFoundError(error)) continue;
      throw error;
    }
    const entry = { name, client, edit };
    await requireCompleteIndex(entry, [source.path, target.path], signal);
    replies.push(entry);
  }
  if (replies.length === 0) throw new Error("No language server supports workspace/willRenameFiles for these paths");
  return replies;
}

function validateDocumentEdits({ edit, client, name }) {
  if (edit === null || edit === undefined) return;
  const encoding = client.serverCapabilities?.positionEncoding;
  if (encoding && encoding !== "utf-16") throw new Error(`${name} uses unsupported position encoding ${encoding}`);
  if (typeof edit !== "object" || Array.isArray(edit) || (edit.changes && edit.documentChanges)) {
    throw new Error(`${name} returned an ambiguous WorkspaceEdit`);
  }
  const validEdits = edits => Array.isArray(edits) && edits.every(item => item?.range && typeof item.newText === "string");
  if (edit.changes && Object.values(edit.changes).some(edits => !validEdits(edits))) {
    throw new Error(`${name} returned unsupported edits`);
  }
  if (edit.documentChanges && !Array.isArray(edit.documentChanges)) throw new Error(`${name} returned invalid document changes`);
  for (const operation of edit.documentChanges ?? []) {
    const document = operation?.textDocument;
    if (operation?.kind || typeof document?.uri !== "string" || !validEdits(operation.edits)) {
      throw new Error(`${name} returned an unsupported WorkspaceEdit operation`);
    }
    const openVersion = client.openFiles.get(document.uri)?.version;
    // gopls marks text read from disk with 0, while OMP starts opened documents at version 1.
    const diskVersion = name === "gopls" && document.version === 0 && openVersion === undefined;
    if (document.version != null && document.version !== openVersion && !diskVersion) {
      throw new Error(`${name} returned a stale document version for ${document.uri}`);
    }
  }
}

function validPosition(lines, position) {
  if (!Number.isSafeInteger(position?.line) || position.line < 0 || position.line >= lines.length ||
      !Number.isSafeInteger(position.character) || position.character < 0) return false;
  const { line, character } = position;
  const text = lines[line];
  const max = text.endsWith("\r") ? text.length - 1 : text.length;
  const next = text.charCodeAt(character);
  const previous = text.charCodeAt(character - 1);
  return character <= max && !(next >= 0xdc00 && next <= 0xdfff && previous >= 0xd800 && previous <= 0xdbff);
}

function validateRanges(snapshot, edits) {
  const lines = snapshot.text.split("\n");
  for (const edit of edits) {
    const { start, end } = edit.range;
    if (!validPosition(lines, start) || !validPosition(lines, end) || start.line > end.line ||
        (start.line === end.line && start.character > end.character) || edit.newText.includes("\0") ||
        !edit.newText.isWellFormed()) {
      throw new Error(`Invalid UTF-16 edit range or text: ${snapshot.name}`);
    }
  }
}

export async function renamePreview(root, agentDir, request) {
  const startedAt = Date.now();
  const applyRoot = renameApplyRoot(root);
  const source = readRenameSnapshot(root, request.file, startedAt);
  const target = request.action === "rename-file-preview" ? checkRenameDestination(root, source.name, request.new_name) : undefined;
  const config = getConfig(root);
  const signal = AbortSignal.timeout(request.timeout * 1000);
  const replies = target
    ? await fileEdits(config, root, source, target, signal)
    : await symbolEdit(config, root, source, request, signal);
  const byUri = new Map();
  for (const reply of replies) {
    validateDocumentEdits(reply);
    if (reply.edit == null) continue;
    for (const [uri, edits] of flattenWorkspaceTextEdits(reply.edit)) {
      if (!byUri.has(uri)) byUri.set(uri, []);
      byUri.get(uri).push(...edits);
    }
  }
  const before = new Map([[source.name, source]]);
  const after = new Map([[source.name, source]]);
  const touched = new Set();
  if (byUri.size > 256) throw new Error("Rename preview exceeds 256 files");
  let editedBytes = source.bytes.length;
  for (const [uri, edits] of byUri) {
    if (!uri.startsWith("file://")) throw new Error(`WorkspaceEdit refers to a non-file URI: ${uri}`);
    const snapshot = readRenameSnapshot(root, fileURLToPath(uri), startedAt);
    if (touched.has(snapshot.name)) throw new Error(`WorkspaceEdit uses multiple URIs for ${snapshot.name}`);
    touched.add(snapshot.name);
    editedBytes += snapshot.bytes.length;
    if (editedBytes > 32 * 1024 * 1024 || edits.length > 50_000) throw new Error("Rename preview exceeds the source or edit limit");
    validateRanges(snapshot, edits);
    const text = applyTextEditsToString(snapshot.text, edits);
    if (text !== snapshot.text) {
      before.set(snapshot.name, snapshot);
      after.set(snapshot.name, { ...snapshot, bytes: encodeReplacement(text, snapshot.bytes) });
    }
  }
  if (target) {
    const moved = after.get(source.name);
    after.delete(source.name);
    after.set(target.name, { ...moved, name: target.name });
  }
  signal.throwIfAborted();
  const bytes = writeRenamePatch(root, applyRoot, agentDir, [...before.values()], [...after.values()],
    target && { source: source.name, destination: target.name });
  return {
    file: source.name, server: replies.map(reply => reply.name).join(", "), status: "completed",
    text: bytes ? "Semantic rename preview" : "Rename produced no changes", patch_bytes: bytes, apply_root: applyRoot,
  };
}
