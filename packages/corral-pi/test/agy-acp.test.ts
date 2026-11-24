import { expect, test } from "bun:test";
import type { AssistantMessageEvent, Model } from "@earendil-works/pi-ai";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { streamAgy } from "../src/agy/stream.ts";
import type { FakeAgyCase } from "./fixtures/fake-agy.ts";

const posixTest = test.skipIf(process.platform === "win32");
const fixture = fileURLToPath(new URL("./fixtures/fake-agy.ts", import.meta.url));
const model: Model<string> = {
  id: "gemini-4-neon-high", name: "Fake Gemini", api: "agy-acp", provider: "agy", baseUrl: "https://unused.invalid",
  reasoning: true, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1_000, maxTokens: 100,
};
const turn = { instructions: "Append these synthetic instructions.", prompt: "A synthetic prompt, then stop." };
const shellQuote = (arg: string) => `'${arg.replaceAll("'", "'\\''")}'`;

type SpawnTrace = { pid: number; cwd: string; argv: string[]; envKeys: string[]; env: Record<string, string> };
type RpcTrace = { jsonrpc: string; id?: number | string; method?: string; params?: unknown; result?: unknown; error?: unknown };

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "ESRCH") throw err;
    return false;
  }
}

async function runFake(scenario: FakeAgyCase, modelId = model.id) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "corral agy's test-")));
  const launchPath = join(root, "launch.json");
  let timer: ReturnType<typeof setTimeout> | undefined;
  let reaped = false;
  try {
    const executable = join(root, "fake agy");
    const home = join(root, "home");
    const cwd = join(root, "workspace");
    mkdirSync(home, { mode: 0o700 });
    mkdirSync(cwd);
    // Use the exact Bun binary instead of searching PATH (which is sandboxed in the child).
    const command = [process.execPath, fixture, scenario, root].map(shellQuote).join(" ");
    writeFileSync(executable, `#!/bin/sh\nexec ${command} "$@"\n`, { mode: 0o700 });
    const abort = new AbortController();
    const observed: unknown[] = [];
    const stream = streamAgy({ executable, cwd, home, setupTimeoutMs: 1_500,
      turnTimeoutMs: scenario === "timeout" ? 100 : 1_500, cancelTimeoutMs: 250 },
      { ...model, id: modelId }, turn, { signal: abort.signal, onProviderStreamEvent: packet => { observed.push(packet); } });
    const collect = async () => {
      const events: AssistantMessageEvent[] = [];
      for await (const event of stream) {
        events.push(event);
        if (scenario === "cancel" && event.type === "text_delta") abort.abort();
      }
      return events;
    };
    const result = stream.result().then(message => {
      const spawn: SpawnTrace = JSON.parse(readFileSync(launchPath, "utf-8"));
      reaped = !processExists(spawn.pid);
      expect(reaped).toBe(true);
      return { message, spawn };
    });
    const deadline = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`fake ACP ${scenario} did not settle`)), 4_000);
    });
    const [{ message, spawn }, events] = await Promise.race([Promise.all([result, collect()]), deadline]);
    const wire: RpcTrace[] = readFileSync(join(root, "wire.ndjson"), "utf-8").trim().split("\n").map(line => JSON.parse(line));
    const requests = wire.filter(entry => entry.method !== undefined);
    return { message, spawn, events, observed, wire, requests, methods: requests.map(entry => entry.method), home, cwd };
  } finally {
    clearTimeout(timer);
    try {
      if (!reaped && existsSync(launchPath)) {
        const { pid }: SpawnTrace = JSON.parse(readFileSync(launchPath, "utf-8"));
        // Emergency cleanup after an assertion or watchdog failure, not the path under test.
        try { process.kill(pid, "SIGKILL"); } catch (err) {
          if ((err as NodeJS.ErrnoException).code !== "ESRCH") throw err;
        }
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
}

posixTest("agy streams fragmented Unicode and thought chunks after confirming the exact model", async () => {
  const { message, spawn, events, wire, requests, methods, home, cwd } = await runFake("stream");
  expect(spawn.cwd).toBe(cwd);
  expect(spawn.argv).toEqual(["acp", "--stdio", "--disable-telemetry", "--sandbox", "--ide-mcp-port=0",
    "--include-default-customizations=false", `--gemini-dir=${join(home, ".gemini")}`]);
  const env = { HOME: home, PATH: "/usr/bin:/bin", LANG: "C.UTF-8", TERM: "dumb", TMPDIR: join(home, "tmp"),
    XDG_CONFIG_HOME: join(home, "config"), XDG_DATA_HOME: join(home, "data"), XDG_STATE_HOME: join(home, "state"),
    XDG_CACHE_HOME: join(home, "cache"), XDG_RUNTIME_DIR: join(home, "runtime") };
  expect(spawn.env).toEqual(env);
  // The /bin/sh fixture wrapper may add bookkeeping variables before exec.
  expect(spawn.envKeys.filter(key => !["PWD", "SHLVL", "_"].includes(key)).sort()).toEqual(Object.keys(env).sort());
  expect(methods).toEqual(["initialize", "session/new", "session/set_model", "session/prompt"]);
  expect(requests[0]).toMatchObject({ jsonrpc: "2.0", params: { protocolVersion: 1 } });
  expect(requests[1]?.params).toEqual({ cwd, mcpServers: [], _meta: { antigravity: { systemPrompt: { append: turn.instructions } } } });
  expect(requests[2]?.params).toEqual({ sessionId: "fake-session", modelId: model.id });
  expect(requests[3]?.params).toEqual({ sessionId: "fake-session", prompt: [{ type: "text", text: turn.prompt }] });
  expect(wire.find(entry => entry.id === "fake-permission")).toEqual({ jsonrpc: "2.0", id: "fake-permission",
    result: { outcome: { outcome: "cancelled" } } });
  expect(wire.find(entry => entry.id === "fake-unknown-method")).toMatchObject({ jsonrpc: "2.0", error: { code: -32601 } });

  expect(events[0]?.type).toBe("start");
  expect(events.at(-1)?.type).toBe("done");
  expect(events.filter(event => event.type.startsWith("thinking_"))).toMatchObject([
    { type: "thinking_start", contentIndex: 0 }, { type: "thinking_delta", contentIndex: 0, delta: "Plan " },
    { type: "thinking_delta", contentIndex: 0, delta: "ready" }, { type: "thinking_end", contentIndex: 0, content: "Plan ready" },
  ]);
  expect(events.filter(event => event.type.startsWith("text_"))).toMatchObject([
    { type: "text_start", contentIndex: 1 }, { type: "text_delta", contentIndex: 1, delta: "Hello 🌍" },
    { type: "text_delta", contentIndex: 1, delta: "!" }, { type: "text_end", contentIndex: 1, content: "Hello 🌍!" },
  ]);
  expect(events.some(event => event.type.startsWith("toolcall_"))).toBe(false);
  expect(message).toMatchObject({ role: "assistant", model: model.id, api: model.api, provider: model.provider, stopReason: "stop",
    content: [{ type: "thinking", thinking: "Plan ready" }, { type: "text", text: "Hello 🌍!" }],
    usage: { input: 4, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 0 } });
  expect(message.diagnostics?.find(diagnostic => diagnostic.type === "agy_acp_usage")?.details).toMatchObject({
    inputTokens: 4, outputTokens: 2, cachedReadTokens: 0, cachedWriteTokens: null,
    thoughtTokens: null, totalTokens: null, costUsd: null,
  });
});

posixTest("agy never prompts a near match or an explicitly mismatched model/effort", async () => {
  const unavailable = await runFake("unavailable");
  expect(unavailable.message.stopReason).toBe("error");
  expect(unavailable.methods).toEqual(["initialize", "session/new"]);
  const mismatch = await runFake("mismatch");
  expect(mismatch.message.stopReason).toBe("error");
  expect(mismatch.methods).toEqual(["initialize", "session/new", "session/set_model"]);
  const currentMismatch = await runFake("current-mismatch");
  expect(currentMismatch.message.stopReason).toBe("error");
  expect(currentMismatch.methods).toEqual(["initialize", "session/new"]);
});

posixTest("agy rejects a base model switch if the server still reports an advertised effort variant", async () => {
  const baseId = "gemini-4-neon";
  const { message, methods, requests } = await runFake("unchanged-variant", baseId);
  expect(methods).toEqual(["initialize", "session/new", "session/set_model"]);
  expect(requests[2]?.params).toEqual({ sessionId: "fake-session", modelId: baseId });
  expect(message.stopReason).toBe("error");
  expect(message.diagnostics?.find(diagnostic => diagnostic.type === "agy_acp_failure")?.details).toEqual({ category: "unavailable" });
});

posixTest("agy abort sends session/cancel and reaps even a TERM-resistant child", async () => {
  const { message, events, requests, methods } = await runFake("cancel");
  // An exact current model needs no session/set_model RPC.
  expect(methods).toEqual(["initialize", "session/new", "session/prompt", "session/cancel"]);
  expect(requests.at(-1)).toEqual({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId: "fake-session" } });
  expect(events.at(-1)?.type).toBe("error");
  expect(message.stopReason).toBe("aborted");
});

posixTest("agy times out and reaps a child that never answers the prompt or cancellation", async () => {
  const { message, methods } = await runFake("timeout");
  expect(methods).toEqual(["initialize", "session/new", "session/prompt", "session/cancel"]);
  expect(message.errorMessage).toContain("prompt timed out");
});

posixTest("agy rejects an unterminated frame as soon as it exceeds the frame bound", async () => {
  const { message } = await runFake("oversize");
  expect(message.errorMessage).toContain("frame too large");
});

for (const scenario of ["auth", "quota", "warning", "bad-error"] as const) {
  const category = scenario === "warning" ? "auth" : scenario === "bad-error" ? "protocol" : scenario;
  posixTest(`agy reports ${category} failure without exposing native output (${scenario})`, async () => {
    const { message, events, observed, methods } = await runFake(scenario);
    expect(methods).toEqual(["initialize", "session/new", "session/prompt"]);
    expect(events.at(-1)?.type).toBe("error");
    expect(message.stopReason).toBe("error");
    expect(message.errorMessage).toBeDefined();
    expect(message.diagnostics?.find(diagnostic => diagnostic.type === "agy_acp_failure")).toMatchObject({
      details: { category, ...(scenario === "warning" ? { source: "executor_warning" } : {}) },
    });
    expect(JSON.stringify({ message, events, observed })).not.toMatch(/FAKE_(?:RPC_MESSAGE|RPC_DATA|STDERR)_DO_NOT_ECHO/);
    if (scenario === "warning") {
      // The agy reply was drained, and input is already net of cache; output includes thoughts.
      expect(message.rawStopReason).toBe("end_turn");
      expect(message.usage).toMatchObject({ input: 7, output: 7, cacheRead: 3, cacheWrite: 0, totalTokens: 17 });
    }
  });
}
