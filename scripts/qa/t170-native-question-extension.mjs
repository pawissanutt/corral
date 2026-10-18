// Fixture-only deterministic provider. The installed engine owns the agent loop,
// tool execution, question registry, real TUI widget, transcript, and control socket.
// Only the model's output is synthetic; no provider request is made.
import assert from "node:assert/strict";
import { createConnection, Socket } from "node:net";
import { dirname, join } from "node:path";
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";

export default async function nativeQuestionFixture(pi) {
  const runtime = process.env.T170_RUNTIME;
  const ipcPath = process.env.T170_NATIVE_IPC;
  assert(runtime && ipcPath, "Only launch this extension through the T-170 fixture");
  assert(process.env.HOME?.startsWith("/tmp/corral-herdr-"), "Private HOME required");
  const { createAskUserTool } = await import(pathToFileURL(join(runtime, "core/extensions/builtin/ask-user/tool.js")).href);
  const { getPendingQuestions } = await import(pathToFileURL(join(runtime, "core/extensions/builtin/ask-user/registry.js")).href);
  const { createAssistantMessageEventStream } = await import(pathToFileURL(
    join(runtime, "../node_modules/@earendil-works/pi-ai/dist/utils/event-stream.js")).href);
  const peer = createConnection(ipcPath);
  const emit = record => peer.write(JSON.stringify(record) + "\n");
  let ctx, tui, registration, token, nextCall, input = "", chain = Promise.resolve();
  const originalWrite = Socket.prototype.write;
  let dropAnswerAck = false;
  Socket.prototype.write = function (data, ...args) {
    // Fault injection wraps only the real server's successful answer ACK. The
    // command has already executed; no native question or server is simulated.
    if (dropAnswerAck) {
      let frame;
      try {
        frame = JSON.parse(typeof data === "string" ? data : Buffer.from(data).toString("utf8"));
      } catch {}
      if (frame?.type === "response" && frame.command === "extension_ui_response" &&
          frame.success === true && typeof frame.id === "string" && frame.id.startsWith("corral-")) {
        dropAnswerAck = false;
        this.destroy();
        emit({ type: "ack-dropped", token, frame });
        return true;
      }
    }
    return originalWrite.call(this, data, ...args);
  };
  const inboxDir = join(dirname(ipcPath), "inbox");
  const register = async () => {
    await mkdir(inboxDir, { recursive: true });
    registration = await pi.session.registerControlEndpoint({
      inboxDir, drain: async () => ({ admitted: [] }), isSessionReferenced: () => true,
    });
    assert.equal(registration.status, "registered", JSON.stringify(registration));
    return registration.socket;
  };
  pi.registerProvider("t170-fixture", {
    api: "t170-local", baseUrl: "http://127.0.0.1:1", apiKey: "fixture-not-a-credential",
    models: [{
      id: "local", name: "T-170 deterministic local fixture", reasoning: false,
      input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000, maxTokens: 1024,
    }],
    streamSimple(model) {
      const stream = createAssistantMessageEventStream();
      const call = nextCall;
      nextCall = undefined;
      const content = call ? [{
        type: "toolCall", id: call, name: "request_user_input",
        arguments: {
          wait_for_answer: false,
          questions: [{
            id: "native_answer", header: "Native QA",
            question: "What exact multiline answer should this native question receive?",
            options: [
              { label: "First", description: "First fixture option." },
              { label: "Second", description: "Second fixture option." },
            ],
          }],
        },
      }] : [{ type: "text", text: "Local fixture turn complete." }];
      const message = {
        role: "assistant", api: model.api, provider: model.provider, model: model.id,
        timestamp: Date.now(), content, stopReason: call ? "toolUse" : "stop",
        usage: {
          input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
      };
      stream.push({ type: "start", partial: message });
      if (call) {
        stream.push({ type: "toolcall_start", contentIndex: 0, partial: message });
        stream.push({ type: "toolcall_end", contentIndex: 0, toolCall: content[0], partial: message });
      }
      stream.push({ type: "done", reason: message.stopReason, message });
      stream.end();
      return stream;
    },
  });
  pi.registerTool(createAskUserTool("codex", pi, { timedOut: false, unavailable: false }));
  pi.events.on("ask-user:asked", ({ request }) => emit({ type: "question", request }));
  pi.events.on("ask-user:closed", event => emit({ type: "closed", ...event }));
  pi.events.on("ask-user:settled", ({ request, response }) => emit({ type: "settled", request, response }));
  pi.on("message_end", event => {
    if (event.message.role === "user" &&
        JSON.stringify(event.message.content).includes(`[Answer to question ${token}]`)) {
      emit({ type: "answer-frame", token, message: event.message });
    }
  });
  pi.on("agent_end", () => emit({ type: "idle", token }));
  pi.on("session_start", async (_event, context) => {
    ctx = context;
    ctx.ui.setWidget("t170-render-barrier", root => {
      tui = root;
      return { render: () => [], invalidate() {} };
    });
    pi.setActiveTools(["request_user_input"]);
    const socket = await register();
    emit({
      type: "ready", pid: process.pid, mode: ctx.mode, cwd: ctx.cwd, socket,
      sessionFile: ctx.sessionManager.getSessionFile(),
    });
  });
  pi.on("session_shutdown", async () => {
    Socket.prototype.write = originalWrite;
    for (const entry of getPendingQuestions(ctx.sessionManager.getSessionId()))
      entry.cancel("Disposable T-170 fixture teardown");
    await registration?.dispose();
    emit({ type: "shutdown", socketWriteRestored: Socket.prototype.write === originalWrite });
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
          case "ask":
            assert(ctx.isIdle(), "Ask only between native turns");
            token = command.token;
            nextCall = token;
            pi.sendUserMessage("Run the deterministic T-170 native question fixture.");
            break;
          case "arm-drop-answer-ack":
            assert(!dropAnswerAck, "Only one acknowledgement fault may be armed");
            dropAnswerAck = true;
            break;
          case "draft":
            ctx.ui.setEditorText("");
            ctx.ui.pasteToEditor(command.text);
            rendered = tui.render(tui.terminal.columns).join("\n");
            tui.renderNow();
            break;
          case "duplicate-draft":
            ctx.ui.setEditorText("");
            ctx.ui.pasteToEditor(command.text);
            ctx.ui.pasteToEditor(command.text);
            rendered = tui.render(tui.terminal.columns).join("\n");
            tui.renderNow();
            break;
          case "clear-fixture-draft":
            ctx.ui.setEditorText("");
            tui.renderNow();
            break;
          case "inspect-draft":
            emit({ type: "draft", token: command.token, text: ctx.ui.getEditorText() });
            break;
          case "shutdown":
            ctx.shutdown();
            break;
          default:
            throw new Error(`Unknown fixture command: ${command.op}`);
        }
        emit({ type: "ack", op: command.op, token: command.token, ...(rendered ? { rendered } : {}) });
      }).catch(error => {
        emit({ type: "error", error: error.stack ?? String(error) });
      });
    }
  });
}
