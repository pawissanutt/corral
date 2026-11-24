import { expect, test } from "bun:test";
import type { AssistantMessageEvent, Model } from "@earendil-works/pi-ai";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createAgentSession, createWriteTool, DefaultResourceLoader, SessionManager, SettingsManager } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { streamAgy } from "../src/agy/stream.ts";
import { AgyToolLane } from "../src/agy/tool-lane.ts";
import { EventLog } from "../src/bridge/events.ts";
import type { BuildContext } from "../src/build/types.ts";
import { installToolControls } from "../src/host/tool-controls.ts";
import { buildParentPolicy, buildShellRefusal, installBuildGuard } from "../src/roles/build/guard.ts";
import reviewer from "../src/roles/reviewer/index.ts";
import type { RoleContext } from "../src/roles/types.ts";
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

type BridgeCase = Extract<FakeAgyCase, `mcp-${string}`>;
const activeTools = (scenario: BridgeCase) => scenario === "mcp-reviewer" ? [...reviewer.upstreamTools]
  : scenario === "mcp-build" ? ["write", "bash"] : ["write"];

async function runThroughPi(scenario: BridgeCase) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "corral-agy-pi-")));
  const cwd = join(root, "checkout"), agentDir = join(root, "pi"), home = join(root, "agy-home");
  mkdirSync(cwd);
  mkdirSync(home, { mode: 0o700 });
  const executable = join(root, "fake-agy");
  const command = [process.execPath, fixture, scenario, root].map(shellQuote).join(" ");
  writeFileSync(executable, `#!/bin/sh\nexec ${command} "$@"\n`, { mode: 0o700 });
  writeFileSync(join(cwd, "review.txt"), "review sentinel");
  writeFileSync(join(root, "denied.txt"), "outside sentinel");
  const writes: string[] = [], commands: string[] = [];
  const entered = Promise.withResolvers<void>();
  const release = Promise.withResolvers<void>();
  const terminalObserved = Promise.withResolvers<void>();
  const releaseObserver = Promise.withResolvers<void>();
  let lane: AgyToolLane | undefined;
  let session: Awaited<ReturnType<typeof createAgentSession>>["session"] | undefined;
  try {
    const build: BuildContext = {
      session: { version: 1, id: "B-test", created: "", updated: "", name: "test", here: true,
        repoRoot: cwd, worktree: cwd, branch: null, base: null, bare: true },
      settings: { exitPolicy: "none", maxParallel: 1, profiles: [] }, dir: root, corral: "fixture-only",
    };
    const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false } });
    const loader = new DefaultResourceLoader({ cwd, agentDir, settingsManager, systemPrompt: turn.instructions,
      noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
      extensionFactories: [pi => {
        if (scenario === "mcp-reviewer") return;
        const write = createWriteTool(cwd);
        pi.registerTool({ ...write, async execute(id, input, signal, update) {
          writes.push(input.path);
          return write.execute(id, input, signal, update);
        } });
        pi.registerTool({ name: "bash", label: "Bash", description: "Fixture command effect",
          parameters: Type.Object({ command: Type.String() }), async execute(_id, input) {
            commands.push(input.command);
            return { content: [{ type: "text", text: "command ran" }], details: undefined };
          } });
        pi.on("tool_call", async (event, ctx) => {
          if (event.toolName === "write" && event.input.path === "pending.txt") {
            entered.resolve();
            await release.promise; // Simulates the owner leaving an approval pending.
            expect(ctx.signal?.aborted).toBe(true);
          }
        });
        installToolControls(pi, {
          ...buildParentPolicy({ build, cwd }, buildShellRefusal, undefined),
          events: new EventLog(), corral: new Set(), currentTurn: () => undefined,
        });
        installBuildGuard(pi, { build, cwd } as RoleContext);
      }],
    });
    await loader.reload();
    const manager = SessionManager.create(cwd, join(root, "sessions"));
    // Pass Pi's auth preflight with a synthetic key for a known provider. The actual
    // stream is replaced below and the model URL cannot reach a real service.
    const sessionModel = { ...model, provider: "openai" };
    ({ session } = await createAgentSession({ cwd, agentDir, settingsManager, model: sessionModel, resourceLoader: loader,
      tools: activeTools(scenario), sessionManager: manager }));
    await session.bindExtensions({ onError: error => { throw error.error; } });
    await session.modelRuntime.setRuntimeApiKey(sessionModel.provider, "fixture-not-a-secret");
    const agent = session.agent;
    lane = new AgyToolLane({ cwd, home, executable, setupTimeoutMs: 1500, turnTimeoutMs: 2000 }, agent);
    agent.streamFunction = lane.stream;
    if (scenario === "mcp-terminal") agent.onProviderStreamEvent = async wire => {
      if ((wire as { result?: { stopReason?: string } }).result?.stopReason !== "end_turn") return;
      terminalObserved.resolve();
      await releaseObserver.promise;
    };
    const run = session.prompt(turn.prompt);
    if (scenario === "mcp-stale" || scenario === "mcp-cancel" || scenario === "mcp-terminal") {
      await Promise.race([entered.promise, run.then(() => { throw new Error(`ACP ended before the approval barrier: ${JSON.stringify(agent.state.errorMessage)}`); })]);
      const signal = agent.signal!;
      const cancelled = new Promise<void>(resolve => signal.addEventListener("abort", () => resolve(), { once: true }));
      if (scenario === "mcp-cancel") agent.abort();
      else writeFileSync(join(root, "mcp-revoke"), "revoke pending call");
      if (scenario === "mcp-terminal") {
        await terminalObserved.promise;
        // A diagnostic consumer may still be waiting, but no approval may execute now.
        expect(signal.aborted).toBe(true);
      } else if (!signal.aborted) await cancelled;
      release.resolve();
    }
    await run;
    await lane.close();
    const file = manager.getSessionFile();
    if (!file) throw new Error("Pi did not persist its transcript");
    const messages = SessionManager.open(file).getBranch().flatMap(entry => entry.type === "message" ? [entry.message] : []);
    const calls = messages.flatMap(message => message.role === "assistant" ? message.content.filter(part => part.type === "toolCall") : []);
    const results = messages.filter(message => message.role === "toolResult");
    if (!existsSync(join(root, "mcp.ndjson"))) throw new Error(`No MCP transcript: ${agent.state.errorMessage ?? "unknown failure"}`);
    const replyLog = readFileSync(join(root, "mcp.ndjson"), "utf8").trim().split("\n")
      .map(line => JSON.parse(line) as { label: string; result: unknown });
    const rpc = Object.fromEntries(replyLog.map(({ label, result }) => [label, result]));
    const wire = readFileSync(join(root, "wire.ndjson"), "utf8").trim().split("\n").map(line => JSON.parse(line));
    const allowed = join(cwd, "allowed.txt");
    return { calls, results, rpc, writes, commands, methods: wire.flatMap(packet => packet.method ?? []),
      allowed: existsSync(allowed) ? readFileSync(allowed, "utf8") : undefined,
      denied: readFileSync(join(root, "denied.txt"), "utf8"), pending: existsSync(join(cwd, "pending.txt")),
      reviewerDenied: existsSync(join(cwd, "reviewer-denied.txt")) };
  } finally {
    release.resolve();
    releaseObserver.resolve();
    await lane?.close();
    session?.dispose();
    rmSync(root, { recursive: true, force: true });
  }
}

posixTest("agy MCP calls are Pi transcript pairs and obey Build file/shell policy exactly once", async () => {
  const { calls, results, rpc, writes, commands, allowed, denied, methods } = await runThroughPi("mcp-build");
  expect(methods).toEqual(["initialize", "session/new", "session/prompt"]);
  expect(rpc.list).toMatchObject({ result: { tools: [{ name: "write" }, { name: "bash" }] } });
  expect(rpc["fixture-error"]).toBeUndefined();
  expect(rpc["native-permission"]).toMatchObject({ outcome: { outcome: "cancelled" } });
  expect(calls.map(call => [call.name, call.arguments])).toEqual([
    ["write", { path: "allowed.txt", content: "once" }],
    ["write", { path: "../denied.txt", content: "no" }],
    ["bash", { command: "git branch -D discard-me" }],
  ]);
  expect(results.map(result => [result.toolCallId, result.isError])).toEqual(calls.map((call, index) => [call.id, index !== 0]));
  expect(writes).toEqual(["allowed.txt"]);
  expect(commands).toEqual([]);
  expect(allowed).toBe("once");
  expect(denied).toBe("outside sentinel");
  for (const [index, name] of ["write", "blocked-write", "blocked-bash"].entries()) {
    expect(rpc[name]).toMatchObject({ result: { isError: index !== 0, content: results[index]!.content } });
  }
  for (const rejected of ["duplicate", "replayed-grant", "wrong-session", "wrong-arguments"]) {
    expect(rpc[rejected]).toMatchObject({ result: { isError: true } });
  }
});

posixTest("agy MCP advertises only the reviewer's active tools, never write", async () => {
  const { calls, results, rpc, reviewerDenied } = await runThroughPi("mcp-reviewer");
  expect(rpc.list).toMatchObject({ result: { tools: reviewer.upstreamTools.map(name => ({ name })) } });
  expect(calls.map(call => call.name)).toEqual(["read"]);
  expect(results).toMatchObject([{ toolCallId: calls[0]!.id, isError: false }]);
  expect(rpc.read).toMatchObject({ result: { content: [{ type: "text", text: "review sentinel" }] } });
  expect(rpc["unlisted-write"]).toMatchObject({ error: { code: -32602 } });
  expect(reviewerDenied).toBe(false);
});

for (const scenario of ["mcp-cancel", "mcp-stale", "mcp-terminal"] as const) {
  posixTest(`agy revocation prevents a still-pending Pi tool effect (${scenario})`, async () => {
    const { calls, results, writes, pending } = await runThroughPi(scenario);
    expect(calls.map(call => call.name)).toEqual(["write"]);
    expect(results).toMatchObject([{ toolCallId: calls[0]!.id, isError: true }]);
    expect(writes).toEqual([]);
    expect(pending).toBe(false);
  });
}
