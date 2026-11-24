import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { parseBuildProfiles } from "../../build/profiles.ts";
import { createSdkRunner, type ChildHooks, type SdkRunnerOptions } from "./child.ts";
import { resolveModel } from "./profiles.ts";

type Handler = (event: Record<string, unknown>, context: { abort(): Promise<void> }) => unknown;
const answer = (text: string): AssistantMessage => ({ role: "assistant", api: "openai-responses", provider: "openai-codex", model: "fixture", timestamp: 1,
  content: [{ type: "text", text }], stopReason: "stop", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } });
const tool = (name: string): ToolDefinition => ({ name, label: name, description: name, parameters: Type.Object({}),
  async execute() { return { content: [{ type: "text", text: name }], details: {} }; } });

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "fleet-child-"));
  const handlers = new Map<string, Handler[]>();
  const registered = new Map<string, ToolDefinition>();
  const active: string[][] = [];
  const messages: AgentMessage[] = [];
  const steers: string[] = [];
  const starts: string[] = [];
  let tools: string[] = [];
  let level: string | undefined;
  let prompt = "";
  let opened = "";
  let aborted = false;
  let body = async () => {};
  let streamTier: unknown;
  const session = {
    sessionId: "child-session", messages,
    agent: { streamFunction(_model: unknown, _context: unknown, options: { serviceTier?: string }) { streamTier = options.serviceTier; } },
    subscribe() { return () => {}; }, async bindExtensions() {},
    async prompt(text: string) { prompt = text; await body(); },
    async abort() { aborted = true; }, async steer(text: string) { steers.push(text); },
    async setModel() {}, setThinkingLevel() {},
    getLastAssistantText() { return messages.filter(message => message.role === "assistant").at(-1)?.content.filter(block => block.type === "text").map(block => block.text).join("\n"); },
    getSessionStats() { return { tokens: { input: 1, output: 1 }, cost: 0 }; },
    getContextUsage() { return undefined; }, dispose() {},
  };
  const sdk = {
    CURRENT_SESSION_VERSION: 3,
    SettingsManager: { inMemory: () => ({}) },
    SessionManager: { open(file: string) { opened = file; return {}; } },
    async createAgentSessionServices(options: { resourceLoaderOptions: { extensionFactories: ((api: ExtensionAPI) => void)[] } }) {
      const api = { registerTool(definition: ToolDefinition) { registered.set(definition.name, definition); },
        on(name: string, handler: Handler) { handlers.set(name, [...handlers.get(name) ?? [], handler]); },
        setActiveTools(names: string[]) { active.push(names); } };
      for (const extension of options.resourceLoaderOptions.extensionFactories) extension(api as unknown as ExtensionAPI);
      return {};
    },
    async createAgentSessionFromServices(options: { tools: string[]; thinkingLevel: string }) {
      tools = options.tools; level = options.thinkingLevel;
      return { session };
    },
  } as unknown as typeof Sdk;
  const hooks: ChildHooks = {
    execution: { sessionFile: join(dir, "transcripts", "research.jsonl"), sessionId: "canonical-child", maxTurns: 10 },
    onEvent() {}, onToolStart(name) { starts.push(name); }, onTurn() {},
    async ask() { return "answer"; }, openingPrompt() { return "Peer messages may arrive."; }, reviewVerdict() {},
    async message() { throw new Error("not used"); }, roster() { return "parent"; },
  };
  const options: SdkRunnerOptions = { sdk, models: { getModel: () => ({}) } as unknown as ModelRuntime,
    context: { dir, session: { worktree: dir, name: "fixture" }, settings: { maxParallel: 1, profiles: [] } },
    agentDir: dir, localTools: [] };
  return { dir, hooks, options, session, messages, registered, active, starts, steers,
    get tools() { return tools; }, get level() { return level; }, get prompt() { return prompt; }, get opened() { return opened; },
    get aborted() { return aborted; }, get streamTier() { return streamTier; },
    body(run: () => Promise<void>) { body = run; },
    async emit(name: string, event: Record<string, unknown> = {}) {
      let result: unknown;
      for (const handler of handlers.get(name) ?? []) result = await handler(event, { abort: () => session.abort() });
      return result;
    },
    cleanup() { rmSync(dir, { recursive: true, force: true }); },
  };
}

test("a Build Explore profile sends Codex priority and keeps its configured effort", async () => {
  const f = fixture();
  const codex = { provider: "openai-codex" };
  const profiles = parseBuildProfiles({ luna: { model: "openai-codex/gpt-6-luna", effort: "low", service_tier: "priority" } });
  const model = resolveModel("luna", profiles, [codex.provider]);
  try {
    f.body(async () => {
      expect(f.level).toBe("low");
      f.session.agent.streamFunction(codex, {}, {});
      expect(f.streamTier).toBe("priority");
      f.session.agent.streamFunction({ provider: "openai" }, {}, {});
      expect(f.streamTier).toBeUndefined();
    });
    const run = createSdkRunner(f.options).run({ id: "explore", title: "Explore", brief: "Read the code", owns: [], kind: "explore" },
      model, { ...f.hooks, execution: undefined }, new AbortController().signal);
    expect((await run.done).failed).toBeUndefined();
  } finally { f.cleanup(); }
});

test("quick research reserves its fourth turn, uses canonical transcript identity and keeps peer messaging", async () => {
  const f = fixture();
  const started = Promise.withResolvers<void>();
  const proceed = Promise.withResolvers<void>();
  f.hooks.execution!.research = { effort: "quick", profile: "docs" };
  f.hooks.execution!.serviceTier = "priority";
  f.body(async () => {
    started.resolve();
    await proceed.promise;
    await f.emit("before_agent_start");
    for (let turn = 1; turn <= 4; turn++) {
      await f.emit("turn_start");
      const context = await f.emit("context", { messages: [] });
      if (turn < 4) {
        expect(context).toBeUndefined();
        expect(f.active).toEqual([]);
      } else {
        expect(f.active).toEqual([[]]);
        expect(context).toMatchObject({ messages: [{ role: "user" }] });
        expect(await f.emit("tool_call", { toolName: "web_search", input: {} })).toMatchObject({ block: true });
      }
      f.messages.push(answer(turn === 4 ? "Findings with citations" : "Research notes"));
      await f.emit("turn_end");
    }
  });
  const handle = createSdkRunner({ ...f.options, bound: true, parentLoadout: ["read", "web_search", "bash", "mcp_docs", "task"],
    localTools: [tool("web_search"), tool("web_fetch"), tool("mcp_docs")] })
    .run({ id: "research", title: "Research", brief: "Find official docs", owns: [], kind: "explore" },
      { profile: "fixture", provider: "openai-codex", model: "fixture" }, f.hooks, new AbortController().signal);
  try {
    await started.promise;
    await handle.steer("Peer evidence: see the official docs.");
    expect(f.steers).toHaveLength(1);
    expect(f.tools).toEqual(["web_search", "agent_message", "agent_roster"]);
    expect([...f.registered.keys()]).toEqual(["web_search", "agent_message", "agent_roster"]);
    expect(f.level).toBe("off");
    f.session.agent.streamFunction({ provider: "openai-codex" }, {}, {});
    expect(f.streamTier).toBe("priority");
    expect(f.prompt).toContain("at most 4 model turns");
    expect(f.opened).toBe(f.hooks.execution!.sessionFile);
    expect(JSON.parse(readFileSync(f.opened, "utf8"))).toMatchObject({ id: "canonical-child" });
    proceed.resolve();
    expect(await handle.done).toMatchObject({ report: "Findings with citations", turns: 4,
      researchReport: { status: "partial", stopReason: "turn_limit" } });
    expect(handle.usage().turns).toBe(4);
    expect(f.aborted).toBe(false);
  } finally { proceed.resolve(); await handle.done; f.cleanup(); }
});

for (const checkout of ["T-17-r2", "T-17-handoff-01234567-r2"]) {
  test(`a bound worker slice may write inside ${checkout} but not beyond its checkout`, async () => {
    const f = fixture();
    try {
      const worktree = join(f.dir, ".worktrees", checkout);
      mkdirSync(worktree, { recursive: true });
      f.body(async () => {
        expect(await f.emit("tool_call", { toolName: "edit", input: { path: "src/file.ts" } })).toBeUndefined();
        expect(await f.emit("tool_call", { toolName: "write", input: { path: "src/new.ts" } })).toBeUndefined();
        for (const path of ["../T-18/src/file.ts", "../marker", "../../outside.ts", ".worktrees/nested/file.ts", ".git/config"]) {
          expect(await f.emit("tool_call", { toolName: "write", input: { path } })).toMatchObject({ block: true });
        }
        f.messages.push(answer("Finished"));
      });
      const handle = createSdkRunner({ ...f.options, bound: true, buildWorktree: true, parentLoadout: ["edit", "write"],
        context: { ...f.options.context, session: { worktree, name: "parent worker" } } })
        .run({ id: "slice", title: "Write", brief: "Edit src/file.ts", owns: ["src/file.ts"], kind: "slice" },
          { profile: "fixture", provider: "openai-codex", model: "fixture" }, f.hooks, new AbortController().signal);
      expect(await handle.done).toMatchObject({ report: "Finished" });
    } finally { f.cleanup(); }
  });
}

test("bound slices intersect SDK base tools and honor the supplied write refusal without loading local overrides", async () => {
  const f = fixture();
  const writes: string[] = [];
  f.body(async () => {
    expect(f.tools).toEqual(["read", "write", "ask_owner", "agent_message", "agent_roster"]);
    expect(f.registered.has("read")).toBe(false);
    expect(f.registered.has("local_exec")).toBe(false);
    expect(f.registered.has("corral")).toBe(false);
    expect(await f.emit("tool_call", { toolName: "write", input: { path: "allowed.ts" } })).toBeUndefined();
    expect(await f.emit("tool_call", { toolName: "write", input: { path: "blocked.ts" } })).toEqual({ block: true, reason: "parent write fence" });
    for (const name of ["edit", "bash", "local_exec", "corral", "mcp_read", "task"]) {
      expect(await f.emit("tool_call", { toolName: name, input: {} })).toMatchObject({ block: true });
    }
    expect(await f.emit("tool_call", { toolName: "read", input: {}, parentToolCallId: "nested" })).toMatchObject({ block: true });
    f.messages.push(answer("Done"));
  });
  try {
    const handle = createSdkRunner({ ...f.options, bound: true,
      parentLoadout: ["read", "write", "local_exec", "corral", "mcp_read", "task"],
      localTools: [tool("read"), tool("local_exec"), tool("corral"), tool("mcp_read"), tool("task")],
      writeRefusal(path) { writes.push(path); return path === "blocked.ts" ? "parent write fence" : undefined; } })
      .run({ id: "slice", title: "Slice", brief: "Implement", owns: [] },
        { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect(await handle.done).toMatchObject({ report: "Done" });
    expect(writes).toEqual(["allowed.ts", "blocked.ts"]);
    expect(f.starts).toEqual(["write"]);
  } finally { f.cleanup(); }
});

test("explore Code Mode is isolated and nested calls obey the read-only child ceiling", async () => {
  const f = fixture();
  f.body(async () => {
    expect(f.tools).toContain("codemode");
    expect(f.registered.has("codemode")).toBe(true);
    expect(await f.emit("tool_call", { toolName: "read", input: {}, parentToolCallId: "cell" })).toBeUndefined();
    for (const name of ["write", "bash", "task", "agent_message", "codemode", "mcp_read"]) {
      expect(await f.emit("tool_call", { toolName: name, input: {}, parentToolCallId: "cell" })).toMatchObject({ block: true });
    }
    await f.emit("before_agent_start");
    for (let turn = 1; turn < 10; turn++) { await f.emit("turn_start"); await f.emit("turn_end"); }
    expect(await f.emit("tool_call", { toolName: "read", input: {}, parentToolCallId: "cell" })).toMatchObject({ block: true });
    f.messages.push(answer("Evidence"));
  });
  try {
    const handle = createSdkRunner({ ...f.options, bound: true,
      parentLoadout: ["read", "codemode", "write", "bash", "task", "mcp_read"] })
      .run({ id: "explore", title: "Explore", brief: "Trace", owns: [], kind: "explore" },
        { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect((await handle.done).failed).toBeUndefined();
  } finally { f.cleanup(); }
});

test("Build Explore inherits Code Mode without expanding its file-only ceiling", async () => {
  const f = fixture();
  f.body(async () => {
    expect(f.tools).toContain("codemode");
    expect(f.registered.has("codemode")).toBe(true);
    expect(await f.emit("tool_call", { toolName: "read", input: {}, parentToolCallId: "cell" })).toBeUndefined();
    for (const name of ["bash", "write", "agent_message", "codemode"]) {
      expect(await f.emit("tool_call", { toolName: name, input: {}, parentToolCallId: "cell" })).toMatchObject({ block: true });
    }
    f.messages.push(answer("Evidence"));
  });
  try {
    const handle = createSdkRunner({ ...f.options, parentLoadout: ["codemode"] })
      .run({ id: "explore", title: "Explore", brief: "Trace", owns: [], kind: "explore" },
        { profile: "fixture", provider: "fixture", model: "fixture" }, { ...f.hooks, execution: undefined }, new AbortController().signal);
    expect((await handle.done).failed).toBeUndefined();
  } finally { f.cleanup(); }
});

test("research reserves a tool-free synthesis request at the deadline", async () => {
  const f = fixture();
  f.hooks.execution!.deadlineAt = 5000;
  f.body(async () => {
    await f.emit("before_agent_start");
    await f.emit("turn_start");
    await f.emit("context", { messages: [] });
    f.messages.push(answer("Summarized findings"));
    await f.emit("turn_end");
  });
  try {
    const handle = createSdkRunner({ ...f.options, bound: true, parentLoadout: ["read"], now: () => 1 })
      .run({ id: "explore", title: "Explore", brief: "Gather evidence", owns: [], kind: "explore" },
        { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect(await handle.done).toMatchObject({ turns: 1, researchReport: { stopReason: "time_reserve" } });
  } finally { f.cleanup(); }
});

test("non-research turn ceilings abort before another model request", async () => {
  const f = fixture();
  f.hooks.execution!.maxTurns = 1;
  f.body(async () => {
    await f.emit("turn_start");
    await f.emit("turn_end");
    await f.emit("turn_start");
    expect(f.aborted).toBe(true);
  });
  try {
    const handle = createSdkRunner(f.options).run({ id: "slice", title: "Slice", brief: "Implement", owns: [] },
      { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect(await handle.done).toMatchObject({ turns: 1, failed: "Child turn limit reached" });
    expect(handle.usage().turns).toBe(1);
  } finally { f.cleanup(); }
});

test("bound reviews retain read-only Git without enabling other shell commands", async () => {
  const f = fixture();
  f.body(async () => {
    expect(f.tools).toContain("bash");
    expect(await f.emit("tool_call", { toolName: "bash", input: { command: "git diff -- src" } })).toBeUndefined();
    for (const command of ["git commit -am change", "git diff > changes.patch", "cat src/a.ts", "git -c core.pager=cat log"]) {
      expect(await f.emit("tool_call", { toolName: "bash", input: { command } })).toMatchObject({ block: true });
    }
    f.messages.push(answer("Reviewed"));
  });
  try {
    const handle = createSdkRunner({ ...f.options, bound: true, parentLoadout: ["read", "bash"] })
      .run({ id: "review", title: "Review", brief: "Review", owns: [], kind: "review" },
        { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect(await handle.done).toMatchObject({ report: "Reviewed" });
    expect(f.starts).toEqual(["bash"]);
  } finally { f.cleanup(); }
});

test("unbound build slices preserve local tools and the existing build write boundary", async () => {
  const f = fixture();
  f.body(async () => {
    expect(f.tools).toContain("local_exec");
    expect(f.registered.has("local_exec")).toBe(true);
    expect(await f.emit("tool_call", { toolName: "write", input: { path: join(f.dir, "src", "a.ts") } })).toBeUndefined();
    expect(await f.emit("tool_call", { toolName: "write", input: { path: join(f.dir, ".git", "config") } })).toMatchObject({ block: true });
    f.messages.push(answer("Done"));
  });
  try {
    const handle = createSdkRunner({ ...f.options, localTools: [tool("local_exec")] })
      .run({ id: "slice", title: "Slice", brief: "Implement", owns: [] },
        { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect(await handle.done).toMatchObject({ report: "Done" });
  } finally { f.cleanup(); }
});
