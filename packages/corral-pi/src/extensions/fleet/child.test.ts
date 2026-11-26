import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import { createAssistantMessageEventStream, getCurrentSystemPrompt, getCurrentTools, type Api, type Model, type AssistantMessage } from "@earendil-works/pi-ai";
import * as Sdk from "@earendil-works/pi-coding-agent";
import { ImageGeneration } from "../image/tool.ts";
import { runAuxiliarySession } from "../subagent/session.ts";
import { PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import type { ExtensionAPI, ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { parseBuildProfiles } from "../../build/profiles.ts";
import { createSdkRunner, type ChildHooks, type SdkRunnerOptions } from "./child.ts";
import { resolveModel } from "./profiles.ts";

test("fleet and auxiliary use only GPT base in the outgoing Pi prompt", async () => {
  const f = fixture();
  try {
    const models = await Sdk.ModelRuntime.create({ authPath: join(f.dir, "auth.json"), modelsPath: null,
      modelsStorePath: join(f.dir, "models-cache.json"), refreshOnCreate: false });
    const model: Model<Api> = { provider: "openai-codex", id: "neutral-fixture", api: "openai-codex-responses",
      name: "Fixture", input: ["text"], baseUrl: "https://unused.invalid", reasoning: false,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 10000, maxTokens: 1000 };
    const fallback: Model<Api> = { ...model, provider: "anthropic-api", api: "anthropic-messages" };
    models.getModel = (provider, id) => [model, fallback].find(candidate => candidate.provider === provider && candidate.id === id);
    models.checkAuth = async () => ({ type: "api_key", source: "fixture" });
    models.getAuth = async () => ({ auth: { apiKey: "fixture" } });
    const outgoing: { prompt: string; tools: string[] }[] = [];
    const sdk: typeof Sdk = { ...Sdk, async createAgentSessionFromServices(options) {
      const built = await Sdk.createAgentSessionFromServices(options);
      built.session.agent.streamFunction = (current, context) => {
        outgoing.push({ prompt: getCurrentSystemPrompt(context.messages), tools: getCurrentTools(context.messages).map(tool => tool.name) });
        const stream = createAssistantMessageEventStream();
        const message = { ...answer("Evidence"), model: current.id, api: current.api, provider: current.provider };
        if (current.provider === model.provider) {
          const quota = { ...message, content: [], stopReason: "error" as const,
            providerFailure: { version: 1, scope: "account", category: "subscription_quota", connection: "openai-subscription" } };
          stream.push({ type: "error", reason: "error", error: quota });
        } else stream.push({ type: "done", reason: "stop", message });
        stream.end();
        return stream;
      };
      return built;
    } };
    const selection = { provider: model.provider, model: model.id };
    const shared = { sdk, models, assetRoot: PACKAGE_ROOT, agentDir: f.dir, localTools: [],
      fallbackPolicy: { children: true, providers: { "openai-codex": ["anthropic-api"] as const } } };
    const fleet = createSdkRunner({ ...f.options, ...shared }).run(
      { id: "review", title: "Review", brief: "Review the evidence", owns: [], kind: "review" },
      { ...selection, profile: "fixture" }, f.hooks, new AbortController().signal);
    expect((await fleet.done).failed).toBeUndefined();
    const aux = await runAuxiliarySession({ ...shared, provider: model.provider, cwd: f.dir, parentLoadout: [] },
      { ...selection, request: "expert", kind: "expert", prompt: "Examine the evidence", maxTurns: 2,
        sessionFile: join(f.dir, "expert.jsonl"), sessionId: "expert-session" }, new AbortController().signal);
    expect(aux.text).toBe("Evidence");
    const asset = (name: string) => readFileSync(join(PACKAGE_ROOT, `assets/families/gpt/${name}.md`), "utf8").trim();
    expect(outgoing).toHaveLength(4);
    for (const [i, contract, expectedTool] of [[0, "Your kind is review.", "review_verdict"],
      [2, "You are a bounded read-only expert", "read"]] as const) {
      const { prompt, tools } = outgoing[i]!;
      expect(prompt).toContain(contract);
      expect(prompt.split(asset("base"))).toHaveLength(2);
      for (const role of ["build", "assistant"]) expect(prompt).not.toContain(asset(role));
      expect(tools).toContain(expectedTool);
      expect(outgoing[i + 1]!.prompt).toContain(contract);
      expect(outgoing[i + 1]!.prompt).not.toContain(asset("base"));
      expect(outgoing[i + 1]!.tools).toEqual(tools);
    }
  } finally { f.cleanup(); }
});

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
  const prompts: string[] = [];
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
    async prompt(text: string) { prompt = text; prompts.push(text); await body(); },
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
  return { dir, hooks, options, session, messages, registered, active, starts, steers, prompts,
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
      f.session.agent.streamFunction(codex, { messages: [] }, {});
      expect(f.streamTier).toBe("priority");
      f.session.agent.streamFunction({ provider: "openai" }, { messages: [] }, {});
      expect(f.streamTier).toBeUndefined();
    });
    const run = createSdkRunner(f.options).run({ id: "explore", title: "Explore", brief: "Read the code", owns: [], kind: "explore" },
      model, { ...f.hooks, execution: undefined }, new AbortController().signal);
    expect((await run.done).failed).toBeUndefined();
  } finally { f.cleanup(); }
});

test("a research preset is a thinking floor and never lowers a stronger profile effort", async () => {
  for (const [effort, research, spec, expected] of [
    ["high", {}, undefined, "high"], // worker task explore: standard used to force low
    ["low", undefined, "deep", "high"], // Build explore asking for deep depth
    [undefined, { effort: "quick" }, undefined, "off"],
  ] as const) {
    const f = fixture();
    try {
      const hooks = research ? { ...f.hooks, execution: { ...f.hooks.execution!, research } } : { ...f.hooks, execution: undefined };
      const run = createSdkRunner({ ...f.options, bound: research !== undefined }).run(
        { id: "explore", title: "Explore", brief: "Read the code", owns: [], kind: "explore", ...(spec ? { effort: spec } : {}) },
        { profile: "fixture", provider: "openai-codex", model: "fixture", ...(effort ? { effort } : {}) }, hooks, new AbortController().signal);
      await run.done;
      expect(f.level).toBe(expected);
    } finally { f.cleanup(); }
  }
});

test("a thin research answer gets one gap check with project orientation, a quick one does not", async () => {
  for (const effort of ["standard", "quick"] as const) {
    const f = fixture();
    mkdirSync(join(f.dir, "src", "lib"), { recursive: true });
    f.hooks.execution!.research = { effort };
    f.body(async () => { f.messages.push(answer(`Answer ${f.prompts.length}`)); });
    try {
      const run = createSdkRunner({ ...f.options, bound: true, parentLoadout: ["read"] }).run(
        { id: "explore", title: "Explore", brief: "Trace the path", owns: [], kind: "explore" },
        { profile: "fixture", provider: "openai-codex", model: "fixture" }, f.hooks, new AbortController().signal);
      const done = await run.done;
      expect(f.prompts[0]).toContain("src/: lib/");
      if (effort === "quick") {
        expect(f.prompts).toHaveLength(1);
      } else {
        expect(f.prompts).toHaveLength(2);
        expect(f.prompts[1]).toContain("Before you finish");
        expect(done).toMatchObject({ report: "Answer 2", researchReport: { status: "completed", depth: { turns: 2, toolCalls: 0 } } });
      }
    } finally { f.cleanup(); }
  }
});

function childImage(onRequest: () => void) {
  return new ImageGeneration({ provider: "openai", model: "gpt-image-1" }, {
    fetch: async () => {
      onRequest();
      return Response.json({ data: [{ b64_json: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==" }] });
    },
    auth: { provider: "openai-codex", providers: ["openai"], credentials: () => ({
      read: async () => ({ type: "api_key", key: "sk-fixtureOnly" }), list: async () => [],
      modify: async () => { throw new Error("unexpected write"); }, delete: async () => {},
    }) },
  }, () => {});
}

test.skipIf(process.platform !== "linux")("slice executes the hosted image binding for its owned file, never the inherited image tool", async () => {
  const f = fixture();
  let requests = 0;
  const image = childImage(() => requests++);
  const signal = new AbortController().signal;
  try {
    f.body(async () => {
      expect(f.tools).not.toContain("web_search");
      expect(f.registered.has("web_search")).toBe(false);
      const execute = (path: string) => f.registered.get("image_gen")!.execute("image", { prompt: "fixture", path }, signal, undefined, {} as never);
      await expect(execute("outside.png")).rejects.toThrow("not owned");
      expect(requests).toBe(0);
      expect((await execute("owned.png")).content[1]).toMatchObject({ type: "image", mimeType: "image/png" });
      expect(readFileSync(join(f.dir, "owned.png")).length).toBeGreaterThan(0);
    });
    const run = createSdkRunner({ ...f.options, parentLoadout: ["image_gen"], localTools: [tool("image_gen"), tool("web_search")],
      hostedTools: ({ kind, owns }) => {
        expect(kind).toBe("slice");
        return [image.tool({ root: f.dir, refusal: path => path && owns.includes(path) ? undefined : "not owned" }), tool("web_search")];
      },
    }).run({ id: "slice", title: "Slice", brief: "Generate an owned fixture", owns: ["owned.png"] },
      { profile: "fixture", provider: "openai-codex", model: "fixture" }, f.hooks, signal);
    expect((await run.done).failed).toBeUndefined();
    expect(requests).toBe(1);
  } finally { image.close(); f.cleanup(); }
});

test("quick research reserves its fourth turn, uses canonical transcript identity and keeps peer messaging", async () => {
  const f = fixture();
  const started = Promise.withResolvers<void>();
  const proceed = Promise.withResolvers<void>();
  let requests = 0;
  const image = childImage(() => requests++);
  f.hooks.execution!.research = { effort: "quick", profile: "docs" };
  f.hooks.execution!.serviceTier = "priority";
  f.body(async () => {
    started.resolve();
    await proceed.promise;
    await f.emit("before_agent_start");
    for (let turn = 1; turn <= 4; turn++) {
      await f.emit("turn_start");
      const context = await f.emit("context", { messages: [] });
      if (turn === 1) {
        const execute = (input: { prompt: string; path?: string }) => f.registered.get("image_gen")!.execute("preview", input, undefined, undefined, {} as never);
        await expect(execute({ prompt: "fixture", path: "owned.png" })).rejects.toThrow("read-only preview");
        expect(requests).toBe(0);
        expect((await execute({ prompt: "fixture" })).content[1]).toMatchObject({ type: "image" });
      }
      if (turn < 4) {
        expect(context).toBeUndefined();
        expect(f.active).toEqual([]);
      } else {
        expect(f.active).toEqual([[]]);
        expect(context).toMatchObject({ messages: [{ role: "user" }] });
        expect(await f.emit("tool_call", { toolName: "web_search", input: {} })).toMatchObject({ block: true });
        expect(await f.emit("tool_call", { toolName: "image_gen", input: {}, parentToolCallId: "nested" })).toMatchObject({ block: true });
      }
      f.messages.push(answer(turn === 4 ? "Findings with citations" : "Research notes"));
      await f.emit("turn_end");
    }
  });
  const handle = createSdkRunner({ ...f.options, bound: true, parentLoadout: ["read", "web_search", "image_gen", "bash", "mcp_docs", "task"],
    localTools: [tool("web_search"), tool("web_fetch"), tool("mcp_docs")],
    hostedTools: () => [image.tool({ refusal: () => undefined }), tool("bash")] })
    .run({ id: "research", title: "Research", brief: "Find official docs", owns: [], kind: "explore" },
      { profile: "fixture", provider: "openai-codex", model: "fixture" }, f.hooks, new AbortController().signal);
  try {
    await started.promise;
    await handle.steer("Peer evidence: see the official docs.");
    expect(f.steers).toHaveLength(1);
    expect(f.tools).toEqual(["web_search", "image_gen", "agent_message", "agent_roster"]);
    expect([...f.registered.keys()]).toEqual(["web_search", "image_gen", "agent_message", "agent_roster"]);
    expect(f.level).toBe("off");
    f.session.agent.streamFunction({ provider: "openai-codex" }, { messages: [] }, {});
    expect(f.streamTier).toBe("priority");
    expect(f.prompt).toContain("at most 4 model turns");
    expect(f.opened).toBe(f.hooks.execution!.sessionFile);
    expect(JSON.parse(readFileSync(f.opened, "utf8"))).toMatchObject({ id: "canonical-child" });
    proceed.resolve();
    expect(await handle.done).toMatchObject({ report: "Findings with citations", turns: 4,
      researchReport: { status: "partial", stopReason: "turn_limit" } });
    expect(requests).toBe(1);
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

test("trusted Code slices can write in worktrees and outside cwd, but not state", async () => {
  const f = fixture();
  mkdirSync(join(f.dir, ".git"));
  symlinkSync(join(f.dir, ".git"), join(f.dir, "state-link"));
  f.body(async () => {
    for (const path of [".worktrees/x/file", "../outside-file"]) {
      for (const toolName of ["edit", "write"]) expect(await f.emit("tool_call", { toolName, input: { path } })).toBeUndefined();
    }
    for (const path of [".git/config", ".corral/tasks/file", ".corral-next/state", "state-link/config"]) {
      expect(await f.emit("tool_call", { toolName: "write", input: { path } })).toMatchObject({ block: true });
    }
    expect(await f.emit("tool_call", { toolName: "bash", input: { command: "git commit -am x" } })).toMatchObject({ block: true });
    f.messages.push(answer("Done"));
  });
  try {
    const handle = createSdkRunner({ ...f.options, context: { ...f.options.context,
      session: { ...f.options.context.session, trustedCode: true } } })
      .run({ id: "slice", title: "Slice", brief: "Implement", owns: [] },
        { profile: "fixture", provider: "fixture", model: "fixture" }, f.hooks, new AbortController().signal);
    expect(await handle.done).toMatchObject({ report: "Done" });
  } finally { f.cleanup(); }
});

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
