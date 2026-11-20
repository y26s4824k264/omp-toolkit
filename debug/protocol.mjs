import { lstatSync, rmdirSync, unlinkSync } from "node:fs";

export const requestLimit = 16 * 1024;
export const responseLimit = 256 * 1024;
export const readyPrefix = "OMP_DEBUG_READY ";

export function privateSession(directory) {
  const info = lstatSync(directory);
  if (!info.isDirectory() || info.uid !== process.getuid() || (info.mode & 0o077) !== 0) {
    throw new Error("Session directory must be private and owned by the current user");
  }
  return info;
}

export function removeSession({ directory, socket }, original) {
  try {
    const current = privateSession(directory);
    if (original.dev !== current.dev || original.ino !== current.ino) return;
    if (lstatSync(socket, { throwIfNoEntry: false })?.isSocket()) unlinkSync(socket);
    rmdirSync(directory);
  } catch (error) {
    if (error.code !== "ENOENT") console.error(`omp-debug: session cleanup: ${error.message}`);
  }
}

export function integer(value, name, minimum, maximum) {
  const result = Number(value);
  if (value === undefined || value === "" || !Number.isSafeInteger(result) || result < minimum || result > maximum) {
    throw new Error(`${name} must be an integer from ${minimum} to ${maximum}`);
  }
  return result;
}

export function readFrame(socket, limit) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let length = 0;
    const interrupted = () => reject(new Error("Connection closed before a complete message"));
    socket.once("error", reject);
    socket.once("close", interrupted);
    socket.once("end", interrupted);
    function onData(chunk) {
      const newline = chunk.indexOf(10);
      const part = newline < 0 ? chunk : chunk.subarray(0, newline);
      length += part.length;
      if (length > limit) {
        socket.off("data", onData);
        reject(new Error(`Message exceeds ${limit} bytes`));
        return;
      }
      chunks.push(part);
      if (newline < 0) return;
      socket.off("data", onData);
      try {
        resolve(JSON.parse(Buffer.concat(chunks, length).toString("utf8")));
      } catch (error) {
        reject(new Error(`Invalid JSON message: ${error.message}`));
      }
    }
    socket.on("data", onData);
  });
}
