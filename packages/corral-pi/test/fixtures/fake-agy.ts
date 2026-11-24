// Disposable ACP v1 peer. It has no network access code or credential inputs.
import { appendFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";

export type FakeAgyCase = "stream" | "cancel" | "timeout" | "unavailable" | "mismatch" | "current-mismatch"
  | "auth" | "quota" | "warning" | "bad-error" | "oversize";

type Id = string | number;
type Packet = { jsonrpc: "2.0"; id?: Id; method?: string };
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

for await (const line of createInterface({ input: process.stdin })) {
  const packet: Packet = JSON.parse(line);
  appendFileSync(join(root, "wire.ndjson"), `${line}\n`);
  const { id, method } = packet;
  switch (method ?? id) {
    case "initialize":
      ack(id, { protocolVersion: 1, agentCapabilities: { _meta: { antigravity: { systemPrompt: { append: true } } } } });
      break;
    case "session/new":
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
      if (stubborn) {
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
      } else if (scenario === "stream") {
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
      break;
    default:
      throw new Error(`unexpected fake ACP packet: ${method ?? id}`);
  }
}
if (!stubborn) clearTimeout(watchdog);
