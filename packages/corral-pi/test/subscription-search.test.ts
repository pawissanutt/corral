import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { zstdDecompressSync } from "node:zlib";
import type { Options } from "@anthropic-ai/claude-agent-sdk";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { CredentialStore } from "@earendil-works/pi-ai";
import type { Fetch } from "../src/auth/types.ts";
import { overrideSdk } from "../src/claude/sdk.ts";
import { RunBudget } from "../src/capabilities/budget.ts";
import { createModelCatalog } from "../src/host/catalog.ts";
import { HostSession, authContext, restrictProvider } from "../src/host/session.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import type { RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { Type } from "typebox";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";
import { AuxiliaryRuntime } from "../src/extensions/subagent/runtime.ts";
import { boundedSearchResult, runSubscriptionSearch } from "../src/extensions/web/subscription.ts";
import { SearchEvidence, record } from "../src/extensions/web/evidence.ts";
import { WebSettingsSchema } from "../src/extensions/web/settings.ts";
import fixtureAuth, { fixtureTransport } from "./fixtures/auth/index.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";

const settings = { hosts: ["*"], max_bytes: 1024, timeout_ms: 5000, max_redirects: 3, search_provider: "subscription" as const };
const temps: string[] = [];
afterEach(() => { overrideSdk(undefined); for (const path of temps.splice(0)) rmSync(path, { recursive: true, force: true }); });
function temp() { const path = mkdtempSync(join(tmpdir(), "corral-search-")); temps.push(path); return path; }
function body(init?: RequestInit) {
  let bytes = typeof init?.body === "string" ? Buffer.from(init.body)
    : init?.body instanceof Uint8Array ? Buffer.from(init.body) : Buffer.alloc(0);
  if (new Headers(init?.headers).get("content-encoding") === "zstd") bytes = zstdDecompressSync(bytes);
  return record(JSON.parse(bytes.toString()));
}
function codexReply(search = true): Response {
  const item = { type: "message", id: "msg", role: "assistant", status: "completed",
    content: [{ type: "output_text", text: "A sourced answer", annotations: [
      { type: "url_citation", title: "Guide", url: "https://docs.example.org/guide" },
    ] }] };
  const events = [
    { type: "response.created", response: { id: "resp_search", status: "in_progress" } },
    ...(search ? [{ type: "response.output_item.done", output_index: 0, item: {
      type: "web_search_call", id: "ws", status: "completed", action: { type: "search", sources: [
        { title: "Guide", url: "https://docs.example.org/guide" },
        { title: "Duplicate", url: "https://docs.example.org/guide" },
        { title: "Reference", url: "https://docs.example.org/reference" },
      ] },
    } }] : []),
    { type: "response.output_item.added", output_index: 1, item: { ...item, content: [] } },
    { type: "response.output_text.delta", output_index: 1, delta: "A sourced answer" },
    { type: "response.output_item.done", output_index: 1, item },
    { type: "response.completed", response: { id: "resp_search", status: "completed", usage: {
      input_tokens: 32, input_tokens_details: { cached_tokens: 2 }, output_tokens: 3, total_tokens: 35,
    } } },
  ];
  return new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""), { headers: { "content-type": "text/event-stream" } });
}
async function fixture(provider: "openai-codex" | "anthropic", send: Fetch, limited = false) {
  const cwd = temp();
  const credentials: CredentialStore = provider === "openai-codex" ? fixtureAuth.credentials(authContext()) : {
    read: async (id) => id === "anthropic" ? { type: "oauth", access: "sk-ant-oat01-SYNTHETIC", refresh: "r", expires: Date.now() + 3600000 } : undefined,
    list: async () => [{ providerId: "anthropic", type: "oauth" }],
    modify: async () => { throw new Error("read-only fixture"); }, delete: async () => {},
  };
  const models = await createModelCatalog(credentials, { providers: [provider], modelsPath: null });
  const network = guardedFetch(send, undefined, provider);
  restrictProvider(models, provider, network.fetch as typeof fetch, {
    configDir: join(cwd, "claude"), cwd, executable: () => "/fixture/claude", searchOnly: true,
    env: { PATH: "/bin", ANTHROPIC_API_KEY: "paid-key", CLAUDE_CODE_OAUTH_TOKEN: "foreign-token" },
  });
  const budgetRecords: Record<string, unknown>[] = [];
  const budget = new RunBudget({ cancel() {}, async record(payload) {
    budgetRecords.push(payload);
    return { admitted: !limited, exhausted: false, limits: { max_tokens: limited ? 100 : null, max_usd_nanos: null } };
  } });
  const restricted = models.getProvider(provider)!;
  models.registerNativeProvider({ ...restricted, streamSimple: budget.wrap(restricted.streamSimple.bind(restricted), true) });
  const refs: Record<string, unknown>[] = [];
  let binding = { provider, model: provider === "openai-codex" ? "gpt-6.1-sol" : "claude-sonnet-5-5" };
  const runtime = new AuxiliaryRuntime({ generation: 4, provider, model: binding.model,
    settings: { web_search: { model: null, timeout_secs: 5 } }, sessionId: () => "parent",
    searchModel: () => binding,
    async record(payload) { refs.push(payload); return { session_id: payload.request, session_file: join(cwd, `${payload.request}.jsonl`) }; },
    execute: (invocation, signal) => runSubscriptionSearch({ sdk, models, cwd, settings }, invocation, signal),
  });
  const run = (id: string, signal?: AbortSignal) => runtime.run("web_search", id, JSON.stringify({ query: "Rust docs", count: 1 }), signal);
  const transcript = () => readFileSync(join(cwd, `${refs[0]?.request}.jsonl`), "utf8").split("\n").filter(Boolean).map((s) => record(JSON.parse(s)));
  return { runtime, run, refs, budgetRecords, transcript, setModel: (model: string) => { binding = { provider, model }; } };
}

test("Codex forces hosted search through OAuth, returns deduplicated sources and persists usage once", async () => {
  let calls = 0;
  const fx = await fixture("openai-codex", async (url, init) => {
    calls++;
    expect(String(url)).toBe("https://chatgpt.com/backend-api/codex/responses");
    expect(new Headers(init?.headers).get("authorization")).toMatch(/^Bearer /);
    const request = body(init);
    expect(request.tools).toEqual([{ type: "web_search", search_context_size: "medium" }]);
    expect(request.tool_choice).toEqual({ type: "web_search" });
    expect(request.include).toEqual(["web_search_call.action.sources"]);
    return codexReply();
  });
  const result = await fx.run("one");
  expect(JSON.parse(result.text)).toMatchObject({ configured: true, provider: "openai-codex", answer: "A sourced answer",
    results: [{ title: "Guide", url: "https://docs.example.org/guide" }], truncated: true });
  expect((await fx.run("one")).request).toBe(result.request);
  expect(calls).toBe(1);
  expect(fx.refs.map((r) => [r.action, r.usage_kind, r.outcome])).toEqual([
    ["prepare", "web_search", undefined], ["started", "web_search", undefined], ["finish", "web_search", "completed"],
  ]);
  const assistant = fx.transcript().map((entry) => record(entry.message)).find((m) => m.role === "assistant");
  expect(record(assistant?.usage)).toMatchObject({ input: 30, output: 3, cacheRead: 2, totalTokens: 35 });
  expect(fx.budgetRecords[0]).toMatchObject({ action: "reserve", unbounded: true });
  expect(fx.budgetRecords[1]).toMatchObject({ action: "settle", tokens: 35 });
  await fx.runtime.close();
});

test("a completion without search evidence is refused, while its usage remains attributable", async () => {
  const fx = await fixture("openai-codex", async () => codexReply(false));
  await expect(fx.run("not-search")).rejects.toThrow("verified hosted search");
  expect(fx.refs.at(-1)?.outcome).toBe("uncertain");
  expect(fx.transcript().some((entry) => record(entry.message).role === "assistant")).toBeTrue();
  expect(fx.budgetRecords.at(-1)?.action).toBe("settle");
});

test("provider errors do not retry or fall back to another transport", async () => {
  let calls = 0;
  const fx = await fixture("openai-codex", async () => { calls++; return Response.json({ error: { message: "quota exceeded" } }, { status: 429 }); });
  await expect(fx.run("quota")).rejects.toThrow();
  expect(calls).toBe(1);
  expect(fx.refs.at(-1)?.outcome).toBe("uncertain");
  expect(fx.budgetRecords).toHaveLength(1);
});

test("hard budgets refuse the unbounded hosted-search transport before network", async () => {
  let calls = 0;
  const fx = await fixture("openai-codex", async () => { calls++; return codexReply(); }, true);
  await expect(fx.run("limited")).rejects.toThrow("budget_exhausted");
  expect(calls).toBe(0);
  expect(fx.budgetRecords[0]?.unbounded).toBeTrue();
});

test("cancellation retains a single uncertain reference and does not retry the model", async () => {
  const wire = fakeCodexTransport();
  wire.script([{ text: "partial" }]); wire.holdNext();
  const held = wire.whenHeld();
  const fx = await fixture("openai-codex", wire.send);
  const cancel = new AbortController();
  const result = fx.run("cancel", cancel.signal);
  const observed = Promise.allSettled([result]);
  await held;
  cancel.abort(new Error("owner cancelled"));
  expect((await observed)[0]?.status).toBe("rejected");
  expect(fx.refs.at(-1)?.outcome).toBe("uncertain");
  expect(wire.requests).toBe(1);
  expect(fx.budgetRecords).toHaveLength(1);
});

function fakeClaude(script: (options: Options, interrupted: Promise<void>) => AsyncGenerator<unknown>) {
  let closes = 0, interrupts = 0;
  overrideSdk({
    createSdkMcpServer: () => { throw new Error("search must not create MCP servers"); },
    query: ({ options }) => {
      const stop = Promise.withResolvers<void>();
      return Object.assign(script(options, stop.promise), {
        interrupt: async () => { interrupts++; stop.resolve(); }, close: () => { closes++; },
      }) as never;
    },
  });
  return { get closes() { return closes; }, get interrupts() { return interrupts; } };
}
function* claudeReply(error = false) {
  yield { type: "assistant", message: { content: [{ type: "tool_use", name: "WebSearch", id: "search-1", input: { query: "Rust docs" } }] } };
  yield { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "search-1", is_error: error,
    content: 'Web search results\nLinks: [{"title":"Rust","url":"https://www.rust-lang.org/"}]' }] } };
  yield { type: "result", subtype: "success", is_error: false, result: "Rust documentation", usage: { input_tokens: 23, output_tokens: 5 } };
}

test("Claude executes only native WebSearch, scrubs ambient auth and accounts the SDK result", async () => {
  const run = fakeClaude(async function* (options) {
    expect(options.tools).toEqual(["WebSearch"]);
    expect(options.allowedTools).toEqual(["WebSearch"]);
    expect(options.settingSources).toEqual([]);
    expect(options.strictMcpConfig).toBeTrue();
    expect(options.maxTurns).toBe(3);
    expect(options.env?.CLAUDE_CODE_OAUTH_TOKEN).toBe("sk-ant-oat01-SYNTHETIC");
    expect(options.env?.ANTHROPIC_API_KEY).toBeUndefined();
    const permissions = { signal: new AbortController().signal, toolUseID: "call-1", requestId: "request-1" };
    expect(await options.canUseTool?.("WebSearch", { query: "docs" }, permissions)).toMatchObject({ behavior: "allow" });
    for (const tool of ["WebFetch", "Bash", "Read", "Write", "Agent", "mcp__foreign__tool"]) {
      expect(await options.canUseTool?.(tool, {}, permissions)).toMatchObject({ behavior: "deny" });
      const hook = options.hooks?.PreToolUse?.[0]?.hooks[0];
      expect(await hook?.({ hook_event_name: "PreToolUse", session_id: "fixture", transcript_path: "/tmp/fixture",
        cwd: "/tmp", tool_name: tool, tool_input: {}, tool_use_id: "call-1" }, "call-1", permissions))
        .toMatchObject({ continue: false, hookSpecificOutput: { permissionDecision: "deny" } });
    }
    yield* claudeReply();
  });
  const fx = await fixture("anthropic", async () => { throw new Error("Claude must not use host HTTP"); });
  expect(JSON.parse((await fx.run("claude")).text)).toMatchObject({ provider: "anthropic", answer: "Rust documentation",
    results: [{ title: "Rust", url: "https://www.rust-lang.org/" }] });
  expect(fx.budgetRecords.at(-1)).toMatchObject({ action: "settle", tokens: 28 });
  expect(run.closes).toBe(1);
});

test("failed native Claude searches are not passed off as verified results", async () => {
  fakeClaude(async function* () { yield* claudeReply(true); });
  const fx = await fixture("anthropic", async () => { throw new Error("unexpected HTTP"); });
  await expect(fx.run("failed-tool")).rejects.toThrow("verified hosted search");
  expect(fx.refs.at(-1)?.outcome).toBe("uncertain");
});

test("Claude terminal failures retain reported usage instead of becoming zero-token transcripts", async () => {
  fakeClaude(async function* () {
    yield { type: "result", subtype: "error_max_turns", is_error: true,
      errors: ["search turn limit"], usage: { input_tokens: 30, output_tokens: 5 } };
  });
  const fx = await fixture("anthropic", async () => { throw new Error("unexpected HTTP"); });
  await expect(fx.run("turn-limit")).rejects.toThrow("search turn limit");
  const assistant = fx.transcript().map((entry) => record(entry.message)).find((m) => m.role === "assistant");
  expect(record(assistant?.usage)).toMatchObject({ input: 30, output: 5, totalTokens: 35 });
  expect(fx.refs.at(-1)?.outcome).toBe("uncertain");
  expect(fx.budgetRecords).toHaveLength(1);
});

test("Claude search cancellation interrupts and closes its subprocess query", async () => {
  const started = Promise.withResolvers<void>();
  const run = fakeClaude(async function* (_options, interrupted) { started.resolve(); await interrupted; });
  const fx = await fixture("anthropic", async () => { throw new Error("unexpected HTTP"); });
  const cancel = new AbortController();
  const pending = fx.run("cancel-claude", cancel.signal);
  const observed = Promise.allSettled([pending]);
  await started.promise; cancel.abort();
  expect((await observed)[0]?.status).toBe("rejected");
  expect(run.interrupts).toBe(1); expect(run.closes).toBe(1);
  expect(fx.refs.at(-1)?.outcome).toBe("uncertain");
});

test("bound host search stays subscription-backed for direct and nested calls across reload", async () => {
  restrictEnvironment(process.env);
  const assets = buildFixtureInstall();
  const cwd = temp();
  let host: HostSession | undefined;
  try {
    const modules = await loadModules(assets.dir, "worker");
    let searches = 0;
    const wire = fixtureTransport();
    const webSettings = { ...settings, searxng_url: "https://must-not-be-used.example/" };
    const network = guardedFetch(async (url, init) => {
      const tools = body(init).tools;
      if (Array.isArray(tools) && tools.some((tool) => record(tool).type === "web_search")) {
        searches++; return codexReply();
      }
      return wire.send(url, init);
    }, undefined, "openai-codex", undefined, { settings: webSettings,
      send: async () => { throw new Error("subscription search must not contact SearXNG"); } });
    const role = workerRole(PACKAGE_ROOT);
    const nested: sdk.ToolDefinition = { name: "fixture_search", label: "Nested search", description: "Nested search fixture",
      parameters: Type.Object({}), async execute(_id, _params, signal, _update, context) {
        return (await context.executeTool("web_search", { query: "nested" }, { signal })).result;
      } };
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-123", run: "R1" };
    const events = new EventLog(1);
    const corral = writeFakeCorral(cwd);
    const client = new CorralClient({ corral: corral.path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", client, events, network,
      capabilities: { catalog: 7, digest: "a".repeat(64), preset: "standard", enabled: ["web.enabled", "budget"], web: webSettings },
      modules: { ...modules, role: { ...role, localToolNames: [...role.localToolNames ?? [], nested.name],
        localTools: (context: RoleContext) => [...role.localTools?.(context) ?? [], nested] } },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    for (const stage of ["startup", "reload"]) {
      if (stage === "reload") await host.session.reload();
      wire.script([
        { toolCalls: [{ callId: `${stage}-search`, name: "web_search", arguments: { query: "direct" } }] },
        { toolCalls: [{ callId: `${stage}-nested`, name: nested.name, arguments: {} }] },
        { text: "Done" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      await host.submit({ text: stage, busyPolicy: "prompt", idempotencyKey: stage, origin: "test" }, `rq-${stage}`);
      await ended; await host.drained();
      const result = host.session.messages.find((message) => message.role === "toolResult" && message.toolCallId.startsWith(`${stage}-search`));
      expect(result).toBeDefined();
      expect(result?.role === "toolResult" && result.isError).toBeFalse();
      const content = result?.role === "toolResult" ? result.content.find((part) => part.type === "text") : undefined;
      expect(JSON.parse(content?.type === "text" ? content.text : "null").provider).toBe("openai-codex");
    }
    expect(searches).toBe(4);
    const records = corral.calls().map((call) => call.envelope.payload);
    expect(records.filter((p) => p.kind === "auxiliary" && p.action === "finish" && p.outcome === "completed")).toHaveLength(4);
    expect(records.filter((p) => p.kind === "budget" && p.action === "reserve" && p.unbounded === true)).toHaveLength(4);
  } finally { host?.session.dispose(); assets.cleanup(); }
}, 30000);

test("bounded JSON remains valid UTF-8 and policy accepts only explicit supported search routes", () => {
  const value = { configured: true as const, provider: "anthropic", model: "m", request: "r", answer: "漢字".repeat(500),
    results: [{ title: "Huge".repeat(100), url: "https://example.org/" }], truncated: false };
  const encoded = boundedSearchResult(value, 200);
  expect(Buffer.byteLength(encoded)).toBeLessThanOrEqual(200);
  expect(JSON.parse(encoded).truncated).toBeTrue();
  expect(encoded).not.toContain("�");
  expect(() => boundedSearchResult(value, 1)).toThrow("web_response_too_large");
  expect(WebSettingsSchema.parse(settings).search_provider).toBe("subscription");
  expect(WebSettingsSchema.safeParse({ ...settings, search_provider: "openai-api" }).success).toBeFalse();
  const evidence = new SearchEvidence();
  evidence.observe("openai-codex", { type: "response.output_item.done", item: { type: "web_search_call", status: "failed" } });
  expect(evidence.searched).toBeFalse();
});
