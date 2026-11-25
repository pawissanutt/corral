import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ModelRuntime } from "@earendil-works/pi-coding-agent";
import { AGENT_JOURNAL_MAX, AGENT_MESSAGE_MAX, AGENT_SEND_BUDGET, MAX_CHILDREN_PER_CALL, MAX_PARALLEL_LIMIT, type BuildContext, type ChildSnapshot, type ChildSpec } from "../../build/types.ts";
import { FleetRuntime, type FleetOptions } from "./runtime.ts";
import type { ChildHandle, ChildHooks, ChildRunner } from "./child.ts";
import { fleetTools, formatChildReport } from "./tools.ts";
import { RetainedChildren } from "../../host/retained-children.ts";
interface ChildToolDetails { readonly children: readonly ChildSnapshot[] }

test("build text deltas do not recompute session usage, while turns and tool events do", async () => {
  const dir = mkdtempSync(join(tmpdir(), "fleet-stream-"));
  const ready = Promise.withResolvers<void>();
  const finished = Promise.withResolvers<void>();
  let observer: ((event: Sdk.AgentSessionEvent) => void) | undefined;
  let turnStart: (() => void) | undefined;
  let statsCalls = 0;
  let contextCalls = 0;
  const session = {
    agent: { streamFunction() { throw new Error("Fixture does not send model requests"); } },
    messages: [],
    subscribe(changed: (event: Sdk.AgentSessionEvent) => void) { observer = changed; return () => { observer = undefined; }; },
    async bindExtensions() {},
    async prompt() { ready.resolve(); await finished.promise; },
    async abort() { finished.resolve(); },
    getLastAssistantText: () => "Done",
    getSessionStats() { statsCalls++; return { tokens: { input: 1, output: 2 }, cost: 0 }; },
    getContextUsage() { contextCalls++; return { percent: 3 }; },
    dispose() {},
  };
  const sdk = {
    CURRENT_SESSION_VERSION: 3,
    SettingsManager: { inMemory: () => ({}) },
    SessionManager: { open: () => ({}) },
    async createAgentSessionServices(options: { resourceLoaderOptions: { extensionFactories: ((api: ExtensionAPI) => void)[] } }) {
      const api = { registerTool() {}, on(name: string, handler: unknown) { if (name === "turn_start") turnStart = handler as () => void; } };
      for (const extension of options.resourceLoaderOptions.extensionFactories) extension(api as unknown as ExtensionAPI);
      return {};
    },
    async createAgentSessionFromServices() { return { session }; },
  } as unknown as typeof Sdk;
  const context: BuildContext = { dir, corral: "corral", settings: { maxParallel: 1, exitPolicy: "none", profiles: [] },
    session: { version: 1, id: "B-test", name: "test", created: "", updated: "", repoRoot: dir, worktree: dir,
      branch: "build/test", base: null, here: true } };
  const runtime = new FleetRuntime({ sdk, models: { getModel: () => ({}) } as unknown as ModelRuntime,
    approved: ["fixture"], parent: { provider: "fixture", model: "model" }, context, agentDir: dir, localTools: [],
    changed() {}, deliverToParent() {}, now: () => 1000 });
  const running = runtime.start("call", [{ id: "child", title: "Child", brief: "Implement", owns: ["src"] }]);
  try {
    await ready.promise;
    expect(turnStart).toBeDefined();
    turnStart!();
    expect([statsCalls, contextCalls]).toEqual([1, 1]);
    const message: AssistantMessage = { role: "assistant", api: "openai-responses", provider: "fixture", model: "model", timestamp: 1,
      content: [{ type: "text", text: "x".repeat(100) }], stopReason: "stop",
      usage: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 3,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    observer!({ type: "message_start", message });
    for (let index = 0; index < 100; index++) observer!({ type: "message_update", message,
      assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "x", partial: message } });
    expect([statsCalls, contextCalls]).toEqual([1, 1]);
    expect(runtime.snapshot().children[0]!.messages[0]!.text).toHaveLength(100);
    observer!({ type: "message_end", message });
    expect([statsCalls, contextCalls]).toEqual([2, 2]);
    const partial: AssistantMessage = { ...message,
      content: [{ type: "toolCall", id: "write-1", name: "write", arguments: { path: "src/a.ts", content: "a" } }] };
    observer!({ type: "message_update", message: partial,
      assistantMessageEvent: { type: "toolcall_start", contentIndex: 0, partial } });
    expect([statsCalls, contextCalls]).toEqual([3, 3]);
    expect(runtime.snapshot().children[0]!.messages.find(item => item.kind === "tool")).toMatchObject({
      tool: "write", phase: "requested", argsComplete: false,
    });
    observer!({ type: "tool_execution_start", toolCallId: "write-1", toolName: "write", args: {} });
    expect([statsCalls, contextCalls]).toEqual([4, 4]);
    observer!({ type: "tool_execution_update", toolCallId: "write-1", toolName: "write", args: {},
      partialResult: { content: [{ type: "text", text: "partial result" }] } });
    expect([statsCalls, contextCalls]).toEqual([5, 5]);
    expect(runtime.snapshot().children[0]!.messages.find(item => item.kind === "tool")).toMatchObject({
      tool: "write", phase: "running", partial: true, text: "partial result",
    });
    observer!({ type: "tool_execution_end", toolCallId: "write-1", toolName: "write", isError: false,
      result: { content: [{ type: "text", text: "File written" }] } });
    expect([statsCalls, contextCalls]).toEqual([6, 6]);
  } finally {
    finished.resolve();
    await running;
    await runtime.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

class FakeRunner implements ChildRunner {
  readonly children = new Map<string, { hooks: ChildHooks; steers: string[]; prompt: string; finish(result: { report: string; failed?: string }): void }>();
  run(spec: ChildSpec, _model: unknown, hooks: ChildHooks, signal: AbortSignal): ChildHandle {
    const done = Promise.withResolvers<{ report: string; failed?: string }>();
    const steers: string[] = [];
    this.children.set(spec.id, { hooks, steers, prompt: hooks.openingPrompt(), finish: done.resolve });
    signal.addEventListener("abort", () => done.resolve({ report: `Cancelled ${spec.id}` }), { once: true });
    return { done: done.promise, usage: () => ({ input: 1, output: 2, turns: 3 }),
      async steer(text) { steers.push(text); }, async setModel() { return "applied"; } };
  }
  finish(id: string, failed?: string): void { this.children.get(id)!.finish({ report: `Report from ${id}`, failed }); }
}
const spec = (id: string, extra: Partial<ChildSpec> = {}): ChildSpec => ({ id, title: id, brief: `Implement ${id}`, owns: ["src"], ...extra });
function fixture(maxParallel = 3, options: Partial<FleetOptions> = {}) {
  const runner = new FakeRunner(), delivered: string[] = [];
  const context: BuildContext = { dir: "/data/build/B-test", corral: "corral", settings: { maxParallel, exitPolicy: "none", profiles: [] },
    session: { version: 1, id: "B-test", name: "test", created: "", updated: "", repoRoot: "/repo", worktree: "/repo",
      branch: "build/test", base: null, here: true } };
  const runtime = new FleetRuntime({ sdk: {} as typeof Sdk, models: { getModels: () => [] } as unknown as ModelRuntime,
    approved: ["fixture"], parent: { provider: "fixture", model: "model" }, context, agentDir: "/data/agent", localTools: [],
    runner, changed() {}, deliverToParent(text) { delivered.push(text); }, now: () => 1000, ...options });
  return { runtime, runner, delivered };
}
async function flush(): Promise<void> { for (let turn = 0; turn < 12; turn++) await Promise.resolve(); }

test("bound children commit lifecycle in order and shutdown finishes the canonical running child", async () => {
  const records: Record<string, unknown>[] = [];
  const { runtime, runner } = fixture(1, { allowedKinds: ["slice"], record: async payload => {
    records.push(payload);
    return { session_file: `/canonical/${payload.request}.jsonl`, session_id: payload.request };
  } });
  try {
    await runtime.start("call", [spec("a")]);
    await flush();
    expect(runner.children.size).toBe(1);
    expect(records.map(record => record.action)).toEqual(["prepare", "started"]);
    expect(records.every(record => record.kind === "auxiliary" && record.usage_kind === "slice")).toBe(true);
    const execution = runner.children.get("a")!.hooks.execution!;
    expect(execution.sessionFile).toBe(`/canonical/${records[0]!.request}.jsonl`);
    expect(execution.sessionId).toBe(records[0]!.request);
    expect(runtime.snapshot().children[0]!.sessionFile).toBe(execution.sessionFile);
    await runtime.close();
    expect(records.map(record => record.action)).toEqual(["prepare", "started", "finish"]);
    expect(records.at(-1)!.outcome).toBe("uncertain");
  } finally { await runtime.close(); }
});

test("bound fleet readback keeps the display id, original transcript and final report after shutdown", async () => {
  const dir = mkdtempSync(join(tmpdir(), "retained-child-"));
  const name = "runtime/auxiliary/canonical.jsonl";
  const file = join(dir, name);
  const options = { retained: new RetainedChildren(dir), record: async (payload: Record<string, unknown>) => ({
    session_file: file, session_id: payload.request,
  }) };
  const { runtime, runner } = fixture(1, options);
  const read = (name: string) => JSON.parse(readFileSync(join(dir, "child-records", name), "utf8"));
  try {
    await runtime.start("call", [spec("slice")]);
    await flush();
    expect(read("slice.json")).toMatchObject({ session_file: name, detail: { state: "running" } });
    mkdirSync(join(dir, "runtime/auxiliary"), { recursive: true });
    const raw = '{"type":"toolResult","content":"original output"}\n';
    writeFileSync(file, raw);
    runner.finish("slice");
    await runtime.wait(["slice"]);
    await runtime.close();
    expect(readFileSync(file, "utf8")).toBe(raw);
    expect(read("slice.json")).toMatchObject({ session_file: name, detail: { state: "done", report: "Report from slice" } });
    expect(read("fleet").children).toMatchObject([{ id: "slice", state: "done" }]);
    const resumed = fixture(1, { ...options, retained: new RetainedChildren(dir) }).runtime;
    await expect(resumed.start("second", [spec("slice")])).rejects.toThrow("belongs to this session's saved history");
    await resumed.close();
  } finally { await runtime.close(); rmSync(dir, { recursive: true, force: true }); }
});

test("a refused child prepare starts nothing and exposes the refusal", async () => {
  const records: Record<string, unknown>[] = [];
  const { runtime, runner } = fixture(1, { record: async payload => {
    records.push(payload);
    throw new Error("auxiliary record refused: capability disabled");
  } });
  try {
    await runtime.start("call", [spec("a")]);
    const [child] = await runtime.wait(["a"]);
    expect(runner.children.size).toBe(0);
    expect(child).toMatchObject({ state: "failed", error: "auxiliary record refused: capability disabled" });
    expect(records.map(record => record.action)).toEqual(["prepare"]);
  } finally { await runtime.close(); }
});

test("closing during prepare records not_started without creating a child", async () => {
  const prepared = Promise.withResolvers<Record<string, unknown>>();
  const records: Record<string, unknown>[] = [];
  const { runtime, runner } = fixture(1, { record: async payload => {
    records.push(payload);
    return payload.action === "prepare" ? prepared.promise : {};
  } });
  await runtime.start("call", [spec("a")]);
  const closing = runtime.close();
  prepared.resolve({ session_file: "/canonical/a.jsonl", session_id: records[0]!.request });
  await closing;
  expect(runner.children.size).toBe(0);
  expect(records.map(record => record.action)).toEqual(["prepare", "finish"]);
  expect(records.at(-1)!.outcome).toBe("not_started");
});

test("task returns live ids immediately and delivers each settlement exactly once, independently of its signal", async () => {
  const { runtime, runner, delivered } = fixture();
  try {
    const controller = new AbortController();
    const tool = fleetTools(runtime).find(tool => tool.name === "task")!;
    const result = await tool.execute("call", { agents: [spec("a"), spec("b"), spec("c")] }, controller.signal, undefined, {} as never);
    // ToolDefinition erases this in-process tool's concrete details type.
    const details = result.details as ChildToolDetails;
    expect(details.children.map(child => [child.id, child.kind, child.state])).toEqual([
      ["a", "slice", "running"], ["b", "slice", "running"], ["c", "slice", "running"],
    ]);
    expect(delivered).toEqual([]);
    controller.abort();
    expect(runtime.snapshot().children.every(child => child.state === "running")).toBe(true);
    runner.children.get("a")!.hooks.onToolStart("edit", { path: "outside.ts" });
    runner.children.get("a")!.hooks.onImageSaved!("generated.png");
    runner.finish("a"); runner.finish("b", "Provider failed"); runtime.cancel("c");
    await flush();
    expect(delivered).toHaveLength(3);
    for (const child of runtime.snapshot().children) expect(delivered.filter(text => text === formatChildReport(child))).toHaveLength(1);
    expect(delivered.find(text => text.includes("Report from a"))).toContain("[outside owns]");
    expect(delivered.find(text => text.includes("Report from a"))).toContain("generated.png");
    await runtime.close(); await flush();
    expect(delivered).toHaveLength(3);
  } finally { await runtime.close(); }
});

test("agent_wait collects final reports without automatic redelivery", async () => {
  const { runtime, runner, delivered } = fixture();
  try {
    await runtime.start("call", [spec("a"), spec("b")]);
    const tool = fleetTools(runtime).find(tool => tool.name === "agent_wait")!;
    const collecting = tool.execute("wait", {}, undefined, undefined, {} as never);
    runner.finish("a"); runner.finish("b");
    const result = await collecting;
    // ToolDefinition erases this in-process tool's concrete details type.
    const details = result.details as ChildToolDetails;
    const children = details.children;
    expect(children.map(child => child.state)).toEqual(["done", "done"]);
    expect(result.content).toEqual([{ type: "text", text: children.map(formatChildReport).join("\n\n") }]);
    await flush();
    expect(delivered).toEqual([]);
  } finally { await runtime.close(); }
});

test("timed-out collection does not swallow a later settlement", async () => {
  const { runtime, runner, delivered } = fixture();
  try {
    await runtime.start("call", [spec("a")]);
    const controller = new AbortController();
    const collecting = runtime.wait(["a"], controller.signal);
    controller.abort();
    expect((await collecting)[0]!.state).toBe("running");
    runner.finish("a"); await flush();
    expect(delivered).toHaveLength(1);
    await expect(runtime.wait(["missing"])).rejects.toThrow("Unknown child");
  } finally { await runtime.close(); }
});

test("sibling messages are framed and journaled without becoming owner steers", async () => {
  const { runtime, runner, delivered } = fixture();
  try {
    await runtime.start("call", [spec("a", { kind: "explore" }), spec("b")]);
    const sent = await runner.children.get("a")!.hooks.message("b", "The API is in src/api.ts");
    expect(sent.message).toMatchObject({ seq: 1, from: "a", to: "b", delivery: "delivered" });
    expect(runner.children.get("b")!.steers[0]).toContain("a (explore)");
    expect(runner.children.get("b")!.steers[0]).toContain("reply_to: 1");
    expect(runtime.snapshot().mail).toEqual([sent.message]);
    expect(runtime.snapshot().children[1]!.steers).toEqual([]);
    await runtime.message("parent", "b", "Parent context");
    expect(runtime.snapshot().children[1]!.steers).toEqual([]);
    await runtime.message("b", "parent", "Question for parent");
    expect(delivered[0]).toContain("b (slice)");
    expect(delivered[0]).toContain("Question for parent");
    await runtime.steer("b", "Owner correction");
    expect(runtime.snapshot().children[1]!.steers).toEqual(["Owner correction"]);
  } finally { await runtime.close(); }
});

test("wait_reply resolves with the recipient's answer and explicit reply_to is journaled", async () => {
  const { runtime } = fixture();
  try {
    await runtime.start("call", [spec("a"), spec("b")]);
    const answer = runtime.message("a", "b", "Which API?", { waitReply: true });
    const seq = runtime.snapshot().mail[0]!.seq;
    await runtime.message("b", "a", "Use the stable API", { replyTo: seq });
    expect(await answer).toMatchObject({ reply: "Use the stable API" });
    expect(runtime.snapshot().mail[1]!.replyTo).toBe(seq);
    const implicit = runtime.message("parent", "a", "Ready?", { waitReply: true });
    await runtime.message("a", "parent", "Ready");
    expect((await implicit).reply).toBe("Ready");
  } finally { await runtime.close(); }
});

test("wait_reply refuses a transitive cycle before it can deadlock", async () => {
  const { runtime } = fixture();
  try {
    await runtime.start("call", [spec("a"), spec("b"), spec("c")]);
    const first = runtime.message("a", "b", "Need b", { waitReply: true });
    const second = runtime.message("b", "c", "Need c", { waitReply: true });
    expect((await runtime.message("c", "a", "Need a", { waitReply: true })).unanswered).toBe("would_deadlock");
    await runtime.close();
    expect((await first).unanswered).toBe("recipient_finished");
    expect((await second).unanswered).toBe("recipient_finished");
  } finally { await runtime.close(); }
});

test("queued recipients read their messages and roster in the opening prompt", async () => {
  const { runtime, runner } = fixture(1);
  try {
    await runtime.start("call", [spec("a"), spec("b")]);
    expect((await runtime.message("a", "b", "Use the shared API")).message.delivery).toBe("queued");
    runner.finish("a"); await flush();
    const prompt = runner.children.get("b")!.prompt;
    expect(prompt).toContain("Use the shared API");
    expect(prompt).toContain("from a (slice)");
    expect(prompt).toContain("b (slice, running)");
    expect(runtime.roster().map(row => [row.id, row.kind, row.state])).toEqual([["parent", "parent", "live"], ["a", "slice", "done"], ["b", "slice", "running"]]);
  } finally { await runtime.close(); }
});

test("finished recipients answer from their reports and pending waits end on settlement, cancellation or timeout", async () => {
  const { runtime, runner } = fixture();
  try {
    await runtime.start("call", [spec("a"), spec("b"), spec("c")]);
    const waiting = runtime.message("parent", "a", "Need result", { waitReply: true });
    runner.finish("a"); await flush();
    expect(await waiting).toMatchObject({ unanswered: "recipient_finished", report: "Report from a" });
    expect(await runtime.message("parent", "a", "Any news?")).toMatchObject({ message: { delivery: "answered_from_report" }, report: "Report from a" });
    const cancelled = runtime.message("b", "c", "Need c", { waitReply: true });
    runtime.cancel("c");
    expect((await cancelled).unanswered).toBe("recipient_finished");
    expect((await runtime.message("parent", "b", "Waiting briefly", { waitReply: true, timeoutMs: 0 })).unanswered).toBe("timeout");
  } finally { await runtime.close(); }
});

test("child send budgets and message bounds are enforced while the parent journal is bounded", async () => {
  const { runtime } = fixture();
  try {
    await runtime.start("call", [spec("a")]);
    await expect(runtime.message("a", "parent", "x".repeat(AGENT_MESSAGE_MAX + 1))).rejects.toThrow("characters");
    for (let index = 0; index < AGENT_SEND_BUDGET; index++) await runtime.message("a", "parent", String(index));
    await expect(runtime.message("a", "parent", "One too many")).rejects.toThrow("send budget");
    for (let index = 0; index <= AGENT_JOURNAL_MAX; index++) await runtime.message("parent", "a", String(index));
    expect(runtime.snapshot().mail).toHaveLength(AGENT_JOURNAL_MAX);
    expect(runtime.snapshot().mail.at(-1)!.text).toBe(String(AGENT_JOURNAL_MAX));
  } finally { await runtime.close(); }
});

test("review verdicts are stored as given and missing verdicts fail instead of looking approved", async () => {
  const { runtime, runner } = fixture();
  try {
    await runtime.start("call", [spec("reviewed", { kind: "review" }), spec("missing", { kind: "review" }), spec("explored", { kind: "explore" })]);
    const verdict = { verdict: "changes" as const, summary: "Missing bound", findings: [{ severity: "major" as const, path: "src/api.ts", line: 3, text: "Unbounded input" }], head: "abc" };
    runner.children.get("reviewed")!.hooks.reviewVerdict(verdict);
    for (const id of ["reviewed", "missing", "explored"]) {
      runner.children.get(id)!.hooks.onToolStart("edit", { path: "outside.ts" });
      runner.finish(id);
    }
    const children = await runtime.wait(["reviewed", "missing", "explored"]);
    expect(children[0]!.verdict).toEqual({ ...verdict, at: 1000 });
    expect(children[0]!.state).toBe("done");
    expect(children[1]).toMatchObject({ state: "failed", error: "Review ended without a verdict" });
    expect(children.every(child => child.edits.length === 0)).toBe(true);
  } finally { await runtime.close(); }
});

test("unknown recipients are undeliverable", async () => {
  const { runtime } = fixture();
  try {
    expect(await runtime.message("parent", "unknown", "Hello", { waitReply: true }))
      .toMatchObject({ message: { delivery: "undeliverable" }, unanswered: "recipient_finished" });
  } finally { await runtime.close(); }
});

test("fleet admits the shared cap and per-call limits rather than the obsolete eight/twelve limits", async () => {
  const { runtime } = fixture(MAX_PARALLEL_LIMIT);
  try {
    const children = await runtime.start("call", Array.from({ length: MAX_CHILDREN_PER_CALL }, (_, index) => spec(`child-${index}`)));
    expect(children).toHaveLength(MAX_CHILDREN_PER_CALL);
    expect(children.every(child => child.state === "running")).toBe(true);
    await expect(runtime.start("oversized", Array.from({ length: MAX_CHILDREN_PER_CALL + 1 }, (_, index) => spec(`extra-${index}`)))).rejects.toThrow("at most");
  } finally { await runtime.close(); }
  expect(() => fixture(MAX_PARALLEL_LIMIT + 1)).toThrow("parallel cap");
});

test("all reaches the parent and live siblings, never the sender or finished agents", async () => {
  const { runtime, runner, delivered } = fixture();
  try {
    await runtime.start("call", [spec("a"), spec("b"), spec("finished")]);
    runner.finish("finished"); await runtime.wait(["finished"]); await flush();
    const before = delivered.length;
    const sent = await runtime.message("a", "all", "Shared context");
    expect(sent.message).toMatchObject({ to: "all", delivery: "delivered" });
    expect(runner.children.get("a")!.steers).toEqual([]);
    expect(runner.children.get("finished")!.steers).toEqual([]);
    expect(runner.children.get("b")!.steers).toHaveLength(1);
    expect(delivered.slice(before)).toHaveLength(1);
    expect(delivered.at(-1)).toContain("Shared context");
    expect(runtime.snapshot().mail).toHaveLength(1);
  } finally { await runtime.close(); }
});

test("closing settles reply waits even while in-session delivery is pending", async () => {
  const dispatch = Promise.withResolvers<void>();
  const started = Promise.withResolvers<void>();
  const { runtime } = fixture(1, { deliverToParent() { started.resolve(); return dispatch.promise; } });
  try {
    await runtime.start("call", [spec("a")]);
    const waiting = runtime.message("a", "parent", "Need an answer", { waitReply: true });
    await started.promise;
    await runtime.close();
    expect((await waiting).unanswered).toBe("recipient_finished");
  } finally { dispatch.resolve(); await runtime.close(); }
});
