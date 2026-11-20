#!/usr/bin/env node
import { existsSync, writeFileSync } from "node:fs";

let seq = 0;
let running = false;
let buffer = Buffer.alloc(0);
let threadsTimer;

function send(message) {
  const body = Buffer.from(JSON.stringify({ ...message, seq: ++seq }));
  process.stdout.write(Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]));
}

function receive({ seq, command }) {
  const respond = (body = {}, success = true) => send({ type: "response", request_seq: seq, command, success, body });
  const event = (name, body) => send({ type: "event", event: name, body });
  switch (command) {
    case "initialize":
      respond();
      break;
    case "launch":
      respond();
      event("stopped", { reason: "entry", allThreadsStopped: true });
      break;
    case "threads":
      writeFileSync("threads-requested", "");
      threadsTimer = setInterval(() => {
        if (!existsSync("release-threads")) return;
        clearInterval(threadsTimer);
        respond({ threads: [{ id: 42, name: "main" }] });
      }, 10);
      break;
    case "continue":
      running = true;
      respond({ allThreadsContinued: true });
      event("continued", { threadId: 42, allThreadsContinued: true });
      break;
    case "pause":
      respond({}, running);
      if (running) {
        running = false;
        event("stopped", { reason: "pause", threadId: 42, allThreadsStopped: true });
      }
      break;
    case "disconnect":
      clearInterval(threadsTimer);
      respond();
      process.stdin.pause();
      break;
    default:
      respond({}, false);
  }
}

process.stdin.on("data", chunk => {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const end = buffer.indexOf("\r\n\r\n");
    if (end < 0) return;
    const length = Number(buffer.subarray(0, end).toString("ascii").split(":")[1]);
    const start = end + 4;
    if (buffer.length < start + length) return;
    const message = JSON.parse(buffer.subarray(start, start + length).toString("utf8"));
    buffer = buffer.subarray(start + length);
    receive(message);
  }
});
