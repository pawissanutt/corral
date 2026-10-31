#!/usr/bin/env bun
// A stand-in for Chrome that speaks just enough DevTools over fds 3 and 4 for one page call (T-613).
// It answers nothing until a file named `started` exists in the tmpRoot above its profile, and fires
// the page's load event only once `loaded` exists there, so a test decides how long each phase takes.
import { createReadStream, createWriteStream, existsSync, watch } from "node:fs";
import { dirname, join } from "node:path";

const profile = process.argv.find((arg) => arg.startsWith("--user-data-dir="))?.slice("--user-data-dir=".length) ?? "";
const tmpRoot = dirname(dirname(profile));
const gate = (name: string) => new Promise<void>((resolve) => {
  const check = () => {
    if (!existsSync(join(tmpRoot, name))) return;
    watcher.close();
    resolve();
  };
  const watcher = watch(tmpRoot, check);
  check();
});
const started = gate("started");
const loaded = gate("loaded");

const output = createWriteStream("", { fd: 4 });
const send = (message: unknown) => output.write(`${JSON.stringify(message)}\0`);
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
let url = "about:blank";

function result(method: string, params: Record<string, unknown>): Record<string, unknown> {
  switch (method) {
    case "Target.createTarget": return { targetId: "fake-target" };
    case "Target.attachToTarget": return { sessionId: "fake-session" };
    case "Page.navigate": url = String(params.url); return { frameId: "fake-frame" };
    case "Page.getFrameTree": return { frameTree: { frame: { url } } };
    case "Runtime.evaluate": return { result: { value: String(params.expression).includes("title") ? "Fake Page" : "fake body" } };
    case "Page.captureScreenshot": return { data: PNG };
    default: return {};
  }
}

let buffer = "";
createReadStream("", { fd: 3, encoding: "utf8" }).on("data", async (chunk) => {
  buffer += chunk;
  for (let end = buffer.indexOf("\0"); end >= 0; end = buffer.indexOf("\0")) {
    const message = JSON.parse(buffer.slice(0, end)) as { id: number; method: string; params?: Record<string, unknown>; sessionId?: string };
    buffer = buffer.slice(end + 1);
    await started;
    send({ id: message.id, result: result(message.method, message.params ?? {}), ...(message.sessionId ? { sessionId: message.sessionId } : {}) });
    if (message.method === "Page.navigate") {
      const session = message.sessionId;
      void loaded.then(() => send({ method: "Page.loadEventFired", params: {}, sessionId: session }));
    }
  }
});
