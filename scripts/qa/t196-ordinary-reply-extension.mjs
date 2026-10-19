// Fixture-only offline provider; the real OmO engine, editor, transcript, and
// authenticated control endpoint remain authoritative. IPC never submits input.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { dirname, join } from "node:path";
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";

export default async function ordinaryReplyFixture(pi) {
  const runtime = process.env.T196_RUNTIME;
  const ipcPath = process.env.T196_ORDINARY_IPC;
  assert(runtime && ipcPath, "Only launch through the T-196 fixture");
  assert(process.env.HOME?.startsWith("/tmp/corral-herdr-"), "Private HOME required");
  const { createAssistantMessageEventStream } = await import(pathToFileURL(
    join(runtime, "../node_modules/@earendil-works/pi-ai/dist/utils/event-stream.js")).href);
  const peer = createConnection(ipcPath);
  const emit = record => peer.write(JSON.stringify({ pid: process.pid, ...record }) + "\n");
  const words = message => typeof message.content === "string" ? message.content
    : message.content.filter(block => block.type === "text").map(block => block.text).join("\n");
  let ctx, tui, registration, inputToken, removeInputListener, input = "", chain = Promise.resolve();
  let userSequence = 0, lastText;
  pi.registerProvider("t196-fixture", {
    api: "t196-local", baseUrl: "http://127.0.0.1:1", apiKey: "fixture-not-a-credential",
    models: [{
      id: "local", name: "T-196 deterministic offline fixture", reasoning: false,
      input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000, maxTokens: 1024,
    }],
    streamSimple(model) {
      const stream = createAssistantMessageEventStream();
      const message = {
        role: "assistant", api: model.api, provider: model.provider, model: model.id,
        timestamp: Date.now(), content: [{ type: "text", text: "Local fixture turn complete." }],
        stopReason: "stop", usage: {
          input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
      };
      stream.push({ type: "start", partial: message });
      stream.push({ type: "done", reason: "stop", message });
      stream.end();
      return stream;
    },
  });
  pi.on("message_end", event => {
    if (event.message.role === "user") {
      lastText = words(event.message);
      emit({ type: "user", sequence: ++userSequence, text: lastText, message: event.message });
    }
  });
  pi.on("agent_end", () => emit({ type: "idle", userSequence, lastText }));
  pi.on("session_start", async (_event, context) => {
    ctx = context;
    ctx.ui.setWidget("t196-render-barrier", root => {
      tui = root;
      removeInputListener ??= root.addInputListener(data => {
        if (!inputToken) return;
        const observed = inputToken;
        observed.before ??= ctx.ui.getEditorText();
        observed.bytes += Buffer.from(data).toString("hex");
        // A terminal paste may arrive in multiple chunks. Observe the complete
        // pre-armed input, never assume one socket/PTY read is the whole event.
        if (observed.expected?.startsWith(observed.bytes) && observed.expected !== observed.bytes) return;
        inputToken = undefined;
        // Observe only: never consume, replace, or synthesize terminal input.
        queueMicrotask(() => emit({ type: "terminal-input-applied", token: observed.token,
          bytes: observed.bytes, before: observed.before, after: ctx.ui.getEditorText() }));
      });
      return { render: () => [], invalidate() {} };
    });
    pi.setActiveTools([]);
    const inboxDir = join(dirname(ipcPath), "inbox");
    await mkdir(inboxDir, { recursive: true });
    registration = await pi.session.registerControlEndpoint({
      inboxDir, drain: async () => ({ admitted: [] }), isSessionReferenced: () => true,
    });
    assert.equal(registration.status, "registered", JSON.stringify(registration));
    emit({ type: "ready", pid: process.pid, mode: ctx.mode, cwd: ctx.cwd,
      socket: registration.socket, sessionFile: ctx.sessionManager.getSessionFile() });
  });
  pi.on("session_shutdown", async () => {
    removeInputListener?.();
    await registration?.dispose();
    emit({ type: "shutdown", pid: process.pid });
    peer.end();
  });
  peer.setEncoding("utf8");
  peer.on("error", error => { throw error; });
  peer.on("data", chunk => {
    input += chunk;
    let end;
    while ((end = input.indexOf("\n")) >= 0) {
      const command = JSON.parse(input.slice(0, end));
      input = input.slice(end + 1);
      chain = chain.then(async () => {
        let rendered;
        switch (command.op) {
          case "arm-terminal-input":
            assert(ctx.isIdle(), "Arm terminal input only between turns");
            assert(!inputToken, "Only one terminal event barrier may be armed");
            inputToken = { token: command.token, expected: command.bytes, bytes: "" };
            break;
          case "draft":
          case "duplicate-draft":
            assert(ctx.isIdle(), "Stage fixture drafts only between turns");
            ctx.ui.setEditorText("");
            ctx.ui.pasteToEditor(command.text);
            if (command.op === "duplicate-draft") ctx.ui.pasteToEditor(command.text);
            rendered = tui.render(tui.terminal.columns).join("\n");
            tui.renderNow();
            break;
          case "clear-fixture-draft":
            assert(ctx.isIdle(), "Clear fixture drafts only between turns");
            ctx.ui.setEditorText("");
            tui.renderNow();
            break;
          case "inspect-draft":
            emit({ type: "draft", token: command.token, text: ctx.ui.getEditorText(),
              rendered: tui.render(tui.terminal.columns).join("\n") });
            break;
          case "shutdown":
            ctx.shutdown();
            break;
          default:
            throw new Error("Unknown fixture operation: " + command.op);
        }
        emit({ type: "ack", op: command.op, token: command.token, ...(rendered ? { rendered } : {}) });
      }).catch(error => emit({ type: "error", error: error.stack ?? String(error) }));
    }
  });
}
