// Disposable ACP v1 peer. Network access is limited to the supplied fixture bridge.
import { appendFileSync, existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";

export type FakeAgyCase = "stream" | "cancel" | "timeout" | "unavailable" | "mismatch" | "current-mismatch"
  | "unchanged-variant" | "auth" | "quota" | "warning" | "bad-error" | "oversize"
  | "mcp-build" | "mcp-reviewer" | "mcp-stale" | "mcp-terminal" | "mcp-cancel" | "mcp-terminate";

type Id = string | number;
type Packet = { jsonrpc: "2.0"; id?: Id; method?: string; params?: any; result?: any };
const [scenario, root, ...argv] = process.argv.slice(2);
if (!scenario || !root) throw new Error("fake agy must be launched by its test wrapper");

const envKeys = Object.keys(process.env);
const env = Object.fromEntries(["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME",
  "XDG_RUNTIME_DIR", "TMPDIR", "PATH", "LANG", "TERM"].map(key => [key, process.env[key]]));
// Never record values of ambient variables, even if the launcher incorrectly forwards them.
writeFileSync(join(root, "launch.json"), JSON.stringify({ pid: process.pid, cwd: process.cwd(), argv, envKeys, env }));

// Refuse to quit on TERM or stdin EOF in the cancellation case. The watchdog is only a
// backstop against an orphaned fixture; the test expects the adapter to reap us much sooner.
const watchdog = setTimeout(() => process.exit(70), 8_000);
const stubborn = scenario === "cancel" || scenario === "timeout";
if (stubborn) process.on("SIGTERM", () => {});

const sessionId = "fake-session";
const exactModel = "gemini-4-neon-high";
const baseModel = "gemini-4-neon";
const encode = (message: object) => `${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`;
const send = (message: object) => process.stdout.write(encode(message));
const ack = (id: Id | undefined, result: object) => send({ id, result });
const update = (sessionUpdate: string, fields: object) => ({
  method: "session/update", params: { sessionId, update: { sessionUpdate, ...fields } },
});
const chunk = (kind: "message" | "thought", text: string) => update(`agent_${kind}_chunk`, { content: { type: "text", text } });
const nativeTool = { toolCallId: "fake-native-tool", title: "Read synthetic fixture", kind: "read", status: "pending" };
const permissionId = "fake-permission";
const unknownId = "fake-unknown-method";
let promptId: Id | undefined;
let remainingFragment: Buffer | undefined;
const bridgeScenario = scenario.startsWith("mcp-");
let bridge: { type: string; name: string; url: string; headers?: Record<string, string> | { name: string; value: string }[] } | undefined;
const permissions = new Map<Id, (result: unknown) => void>();
let pendingFetch: AbortController | undefined;
const recordMcp = (label: string, result: unknown) => appendFileSync(join(root, "mcp.ndjson"),
  `${JSON.stringify({ label, result })}\n`);

async function requestPermission(serverName: string, name: string, args: object, toolCallId: string) {
  const id = `permission-${toolCallId}`;
  const response = new Promise<unknown>(resolve => permissions.set(id, resolve));
  send({ id, method: "session/request_permission", params: { sessionId,
    toolCall: { toolCallId, name: `mcp_${serverName}_${name}`, rawInput: { ...args, toolSummary: "fixture" } },
    options: [{ kind: "allow_once", optionId: "allow" }, { kind: "reject_once", optionId: "reject" }],
  } });
  return response;
}

async function runBridge() {
  if (!bridge || bridge.type !== "http" || !/^corral-[a-f0-9]{16}$/.test(bridge.name)) throw new Error("missing HTTP corral bridge");
  const descriptor = bridge;
  const headers = new Headers({ "content-type": "application/json", accept: "application/json, text/event-stream" });
  if (Array.isArray(descriptor.headers)) {
    for (const header of descriptor.headers) headers.set(header.name, header.value);
  } else if (descriptor.headers) {
    for (const [name, value] of Object.entries(descriptor.headers)) headers.set(name, value);
  }
  async function post(body: object, signal?: AbortSignal) {
    const response = await fetch(descriptor.url, { method: "POST", headers, body: JSON.stringify(body), signal });
    const text = await response.text();
    return text ? JSON.parse(text) : null;
  }
  const rpc = (id: number, method: string, params: object) => ({ jsonrpc: "2.0", id, method, params });
  await post(rpc(1, "initialize", { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "fake-agy", version: "1" } }));
  await post({ jsonrpc: "2.0", method: "notifications/initialized" });
  recordMcp("list", await post(rpc(2, "tools/list", {})));
  let nextId = 3;
  const callPacket = (id: number, name: string, args: object, conversationId = sessionId) => rpc(id, "tools/call", {
    name, arguments: args, _meta: { "antigravity.google/conversation_id": conversationId },
  });
  async function call(label: string, name: string, args: object, toolCallId: string) {
    await requestPermission(descriptor.name, name, args, toolCallId);
    const packet = callPacket(nextId++, name, args);
    recordMcp(label, await post(packet));
    return packet;
  }
  let terminalSent = false;
  if (scenario === "mcp-build") {
    const nativeId = "permission-native-write";
    const nativeResponse = new Promise<unknown>(resolve => permissions.set(nativeId, resolve));
    const nativeWrite = { toolCallId: "native-write", name: "write_file", title: "Native write", kind: "edit", status: "pending",
      rawInput: { path: "native-denied.txt", content: "no" } };
    send(update("tool_call", nativeWrite));
    send({ id: nativeId, method: "session/request_permission", params: { sessionId, toolCall: nativeWrite,
      options: [{ kind: "allow_once", optionId: "allow" }, { kind: "reject_once", optionId: "reject" }] } });
    recordMcp("native-permission", await nativeResponse);
    const args = { path: "allowed.txt", content: "once" };
    await requestPermission(descriptor.name, "write", args, "write-allowed");
    recordMcp("wrong-session", await post(callPacket(nextId++, "write", args, "stale-session")));
    recordMcp("wrong-arguments", await post(callPacket(nextId++, "write", { ...args, content: "not granted" })));
    const original = callPacket(nextId++, "write", args);
    recordMcp("write", await post(original));
    recordMcp("duplicate", await post(original));
    await requestPermission(descriptor.name, "write", args, "write-identical-second-grant");
    recordMcp("replayed-grant", await post(callPacket(nextId++, "write", args)));
    await call("blocked-write", "write", { path: "../denied.txt", content: "no" }, "write-blocked");
    await call("blocked-bash", "bash", { command: "git branch -D discard-me" }, "bash-blocked");
  } else if (scenario === "mcp-reviewer") {
    await call("read", "read", { path: "review.txt" }, "read-review");
    await call("unlisted-write", "write", { path: "reviewer-denied.txt", content: "no" }, "write-unlisted");
  } else if (scenario === "mcp-terminate") {
    await call("terminate", "write", { path: "allowed.txt", content: "once" }, "write-terminates-run");
    send(chunk("message", "ACP follow-up after a terminal Pi tool"));
  } else {
    const args = { path: "pending.txt", content: "pending" };
    await requestPermission(descriptor.name, "write", args, "write-pending");
    pendingFetch = new AbortController();
    send(chunk("message", "mcp pending write"));
    let timer: ReturnType<typeof setInterval> | undefined;
    if (scenario === "mcp-stale" || scenario === "mcp-terminal") {
      const revoke = join(root!, "mcp-revoke");
      timer = setInterval(() => {
        if (!existsSync(revoke)) return;
        clearInterval(timer);
        if (scenario === "mcp-stale") pendingFetch?.abort();
        else {
          terminalSent = true;
          ack(promptId, { stopReason: "end_turn" });
        }
      }, 10);
    }
    try {
      recordMcp("pending-write", await post(callPacket(nextId++, "write", args), pendingFetch.signal));
    } catch (error) {
      recordMcp("pending-write", { aborted: pendingFetch.signal.aborted, error: String(error) });
    } finally {
      if (timer) clearInterval(timer);
      pendingFetch = undefined;
    }
  }
  if (!terminalSent) ack(promptId, { stopReason: "end_turn" });
}

for await (const line of createInterface({ input: process.stdin })) {
  const packet: Packet = JSON.parse(line);
  appendFileSync(join(root, "wire.ndjson"), `${line}\n`);
  const { id, method } = packet;
  if (!method && id !== undefined && permissions.has(id)) {
    const resolve = permissions.get(id)!;
    permissions.delete(id);
    resolve(packet.result);
    continue;
  }
  switch (method ?? id) {
    case "initialize":
      ack(id, { protocolVersion: 1, agentCapabilities: { _meta: { antigravity: { systemPrompt: { append: true } } } } });
      break;
    case "session/new":
      if (bridgeScenario) bridge = packet.params?.mcpServers?.[0];
      ack(id, { sessionId, models: {
        currentModelId: ["stream", "mismatch", "unavailable"].includes(scenario) ? baseModel : exactModel,
        availableModels: (scenario === "unavailable" ? [baseModel] : [baseModel, exactModel]).map(modelId => ({ modelId })),
      }, ...(scenario === "current-mismatch" ? { configOptions: [{ id: "thought_level", currentValue: "low" }] } : {}) });
      break;
    case "session/set_model":
      // agy 1.5.14 acknowledges the RPC with an empty result and reports its choice by notification.
      ack(id, {});
      send(update("config_option_update", { configOptions: [{ id: "model", currentValue: baseModel },
        { id: "thought_level", currentValue: scenario === "mismatch" ? "low" : "high" }] }));
      break;
    case "session/prompt":
      promptId = id;
      if (bridgeScenario) {
        // Keep consuming ACP while permission responses and cancellation arrive.
        void runBridge().catch(error => {
          recordMcp("fixture-error", String(error));
          send({ id, error: { code: -32603, message: "fixture bridge failed" } });
        });
      } else if (stubborn) {
        send(chunk("message", "waiting for cancel"));
      } else if (scenario === "oversize") {
        // Keep the pipe open and never send a newline: reject while buffering the frame.
        process.stdout.write("x".repeat(1024 * 1024 + 1));
      } else if (scenario === "warning") {
        send(chunk("message", "\n⚠ agent executor error: generating and executing: Error 401, " +
          "Message: FAKE_RPC_MESSAGE_DO_NOT_ECHO, Status: UNAUTHENTICATED, Details: []\n"));
        ack(id, { stopReason: "end_turn", usage: {
          inputTokens: 7, outputTokens: 7, cachedReadTokens: 3, cachedWriteTokens: 0, thoughtTokens: 2, totalTokens: 17,
        } });
      } else if (scenario === "bad-error") {
        send({ id, error: ["FAKE_RPC_DATA_DO_NOT_ECHO"] });
      } else if (scenario === "auth" || scenario === "quota") {
        process.stderr.write("FAKE_STDERR_DO_NOT_ECHO\n");
        const category = scenario === "auth" ? "Authentication required" : "RESOURCE_EXHAUSTED";
        send({ id, error: { code: scenario === "auth" ? -32000 : 429,
          message: `${category}: FAKE_RPC_MESSAGE_DO_NOT_ECHO`, data: { privateDetail: "FAKE_RPC_DATA_DO_NOT_ECHO" } } });
      } else if (scenario === "stream" || scenario === "unchanged-variant") {
        send(chunk("thought", "Plan "));
        send(chunk("thought", "ready"));
        send(update("tool_call", nativeTool));
        send(update("usage_update", { used: 900, size: 1_000, usage: { inputTokens: 900, outputTokens: 90 } }));
        const firstText = Buffer.from(encode(chunk("message", "Hello 🌍")));
        const split = firstText.indexOf(Buffer.from("🌍")) + 2;
        remainingFragment = firstText.subarray(split);
        // Hold the second half of the line until the host responds to the preceding callback.
        // This splits a four-byte code point without relying on a sleep or on pipe chunk size.
        process.stdout.write(Buffer.concat([Buffer.from(encode({ id: permissionId, method: "session/request_permission",
          params: { sessionId, toolCall: nativeTool, options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }] } })),
          firstText.subarray(0, split)]));
      } else {
        throw new Error("fake agy received a prompt without the requested model");
      }
      break;
    case permissionId:
      if (!remainingFragment) throw new Error("unexpected permission response");
      process.stdout.write(remainingFragment);
      send(chunk("message", "!"));
      send({ id: unknownId, method: "fs/read_text_file", params: { sessionId, path: "/synthetic-fixture" } });
      break;
    case unknownId:
      ack(promptId, { stopReason: "end_turn", usage: { inputTokens: 4, outputTokens: 2, cachedReadTokens: 0 } });
      break;
    case "session/cancel":
      // The wire log records receipt before this reply, even if the adapter kills us immediately afterwards.
      if (scenario === "cancel") ack(promptId, { stopReason: "cancelled" });
      if (scenario === "mcp-cancel") pendingFetch?.abort();
      break;
    default:
      throw new Error(`unexpected fake ACP packet: ${method ?? id}`);
  }
}
if (!stubborn) clearTimeout(watchdog);
