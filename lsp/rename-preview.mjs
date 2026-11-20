import { fileURLToPath, pathToFileURL } from "node:url";
import { symbolPosition } from "./client.mjs";
import { selectServers } from "./servers.mjs";
import {
  checkRenameDestination, encodeReplacement, readRenameSnapshot, renameApplyRoot, writeRenamePatch,
} from "./rename-patch.mjs";

const symbolMethod = "textDocument/rename";
const moveMethod = "workspace/willRenameFiles";

function openSource(client, source) {
  const document = client.open(source.path);
  if (document.text !== source.text) throw new Error("Source changed during language-server lookup: " + source.name);
  return document;
}

async function symbolEdit(definitions, root, source, request, signal, connect) {
  const failures = [];
  for (const server of selectServers(definitions, root, source.path, request.action)) {
    signal.throwIfAborted();
    try {
      const client = await connect(server, signal);
      if (!client.capabilities.renameProvider && !client.hasCapability(symbolMethod)) continue;
      const document = openSource(client, source);
      const position = symbolPosition(document.text, request.symbol, request.line);
      // Wait for configured TypeScript projects before asking for cross-file edits.
      await client.waitForIndex(signal, source.path);
      const edit = await client.request(symbolMethod, {
        textDocument: { uri: document.uri }, position, newName: request.new_name,
      }, signal);
      await client.waitForIndex(signal);
      return [{ name: server.name, client, edit }];
    } catch (error) {
      failures.push(server.name + ": " + error.message);
    }
  }
  throw new Error(failures.length ? failures.join("\n") : "No language server supports symbol rename for the source file");
}

async function fileEdits(definitions, root, source, target, signal, connect) {
  const sourceServers = selectServers(definitions, root, source.path, "rename-file-preview");
  const targetServers = selectServers(definitions, root, target.path, "rename-file-preview");
  const servers = new Map([...sourceServers, ...targetServers].map(server => [server.name + "\0" + server.root, server]));
  const files = [{ oldUri: pathToFileURL(source.path).href, newUri: pathToFileURL(target.path).href }];
  const replies = [];
  for (const server of servers.values()) {
    signal.throwIfAborted();
    const client = await connect(server, signal);
    if (!client.capabilities.workspace?.fileOperations?.willRename && !client.hasCapability(moveMethod)) continue;
    if (sourceServers.some(candidate => candidate.name === server.name && candidate.root === server.root)) {
      openSource(client, source);
      await client.waitForIndex(signal, source.path);
    } else {
      await client.waitForIndex(signal);
    }
    let edit;
    try {
      edit = await client.request(moveMethod, { files }, signal);
    } catch (error) {
      if (error.code === -32601) continue;
      throw error;
    }
    await client.waitForIndex(signal);
    replies.push({ name: server.name, client, edit });
  }
  if (!replies.length) throw new Error("No language server supports workspace/willRenameFiles for these paths");
  return replies;
}

function flattenWorkspaceTextEdits(edit) {
  if (edit.changes) return Object.entries(edit.changes);
  return (edit.documentChanges ?? []).map(operation => [operation.textDocument.uri, operation.edits]);
}

function applyTextEditsToString(text, edits) {
  const offsets = [0];
  for (let index = text.indexOf("\n"); index >= 0; index = text.indexOf("\n", index + 1)) offsets.push(index + 1);
  const indexOf = position => offsets[position.line] + position.character;
  const ranges = edits.map(({ range, newText }) => ({
    start: indexOf(range.start), end: indexOf(range.end), newText,
  })).sort((a, b) => a.start - b.start || a.end - b.end);
  const parts = [];
  let offset = 0;
  for (const range of ranges) {
    if (range.start < offset) throw new Error("Language server returned overlapping text edits");
    parts.push(text.slice(offset, range.start), range.newText);
    offset = range.end;
  }
  parts.push(text.slice(offset));
  return parts.join("");
}

function validateDocumentEdits({ edit, client, name }) {
  if (edit === null || edit === undefined) return;
  const encoding = client.capabilities.positionEncoding;
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

export async function renamePreview(definitions, root, agentDir, request, signal, connect) {
  const startedAt = Date.now();
  const applyRoot = renameApplyRoot(root);
  const source = readRenameSnapshot(root, request.file, startedAt);
  const target = request.action === "rename-file-preview" ? checkRenameDestination(root, source.name, request.new_name) : undefined;
  const replies = target
    ? await fileEdits(definitions, root, source, target, signal, connect)
    : await symbolEdit(definitions, root, source, request, signal, connect);
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
