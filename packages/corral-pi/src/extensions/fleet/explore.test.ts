import { afterEach, expect, jest, test } from "bun:test";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { ChildSpec } from "../../build/types.ts";
import { workspaceSettings } from "../../host/config.ts";
import { exploreTool } from "../subagent/index.ts";
import { researchBudget } from "../subagent/research.ts";
import { auxiliarySettings, ExploreSettingsSchema, type ExploreSettings } from "../subagent/settings.ts";
import type { ChildHandle, ChildHooks } from "./child.ts";
import type { ResolvedModel } from "./profiles.ts";
import { FleetRuntime } from "./runtime.ts";

type ResearchResult = Awaited<ChildHandle["done"]>;
interface ResearchCall { spec: ChildSpec; model: ResolvedModel; hooks: ChildHooks }
const settings: ExploreSettings = { max_parallel: 3, model: null, max_turns: 12, timeout_secs: 30 };
function fixture(execute: (call: ResearchCall, signal: AbortSignal) => Promise<ResearchResult>, explore = settings, provider = "openai-codex") {
  const records: Record<string, unknown>[] = [];
  const calls: ResearchCall[] = [];
  const runtime = new FleetRuntime({ sdk: {} as typeof Sdk, models: {} as ModelRuntime,
    approved: [provider], parent: { provider, model: "normal" }, agentDir: "/unused", localTools: [],
    context: { dir: "/private", session: { worktree: "/repo", name: "parent" }, settings: { maxParallel: explore.max_parallel, profiles: [] } },
    allowedKinds: ["explore"], research: { settings: explore },
    changed() {}, deliverToParent() { throw new Error("Blocking research must not be redelivered"); },
    async record(payload) {
      records.push(payload);
      return { session_file: `/private/${payload.request}.jsonl`, session_id: payload.request };
    },
    runner: { run(spec, model, hooks, signal) {
      const call = { spec, model, hooks };
      calls.push(call);
      return { done: execute(call, signal), usage: () => ({ input: 1, output: 2, turns: 3 }),
        async steer() {}, async setModel() { return "applied"; } };
    } },
  });
  return { runtime, calls, records };
}
function cancelled(signal: AbortSignal): Promise<ResearchResult> {
  const { promise, resolve } = Promise.withResolvers<ResearchResult>();
  signal.addEventListener("abort", () => resolve({ report: "cancelled" }), { once: true });
  return promise;
}
afterEach(() => { jest.useRealTimers(); });

test("four explore requests queue at three, preserve attribution, deduplicate and close children", async () => {
  const three = Promise.withResolvers<void>(), four = Promise.withResolvers<void>(), first = Promise.withResolvers<ResearchResult>();
  const fx = fixture((call, signal) => {
    if (fx.calls.length === 3) three.resolve();
    if (fx.calls.length === 4) four.resolve();
    return call.spec.brief === "0" ? first.promise : cancelled(signal);
  });
  const requests = [0, 1, 2, 3].map(id => fx.runtime.explore(String(id), String(id)));
  expect(fx.runtime.explore("0", "duplicate")).toBe(requests[0]!);
  try {
    await three.promise;
    expect(fx.calls).toHaveLength(3);
    expect(fx.runtime.snapshot().children.map(child => child.state)).toEqual(["running", "running", "running", "queued"]);
    first.resolve({ report: "0", researchReport: { status: "completed", stopReason: "completed" } });
    await four.promise;
    expect((await requests[0])?.request).toBe(fx.records[0]?.request);
    expect(fx.runtime.snapshot().children.filter(child => child.state === "running")).toHaveLength(3);
    await fx.runtime.close();
    const results = await Promise.all(requests);
    expect(results.map(result => result.outcome)).toEqual(["completed", "uncertain", "uncertain", "uncertain"]);
    expect(fx.records.filter(record => record.action === "prepare")).toHaveLength(4);
    expect(fx.records.filter(record => record.action === "finish" && record.outcome === "uncertain")).toHaveLength(3);
    expect(fx.runtime.snapshot().children.map(child => child.state)).toEqual(["done", "cancelled", "cancelled", "cancelled"]);
    for (const [index, call] of fx.calls.entries()) {
      expect(call.hooks.execution?.sessionId).toBe(results[index]!.request);
      expect(call.hooks.execution?.sessionFile).toBe(`/private/${results[index]!.request}.jsonl`);
      expect(results[index]!.sessionFile).toBe(call.hooks.execution?.sessionFile);
    }
    await expect(fx.runtime.explore("new", "no")).rejects.toThrow("closed");
  } finally { first.resolve({ report: "0" }); await fx.runtime.close(); }
});

test("fresh fleets allocate distinct transcripts even when tool and child IDs are reused", async () => {
  const answer = async (): Promise<ResearchResult> => ({ report: "answer", researchReport: { status: "completed", stopReason: "completed" } });
  const launch = async () => {
    const fx = fixture(answer);
    try {
      const research = await fx.runtime.explore("research", "question");
      await fx.runtime.start("task", [{ id: "named", kind: "explore", title: "Question", brief: "question", owns: [] }]);
      const [named] = await fx.runtime.wait(["named"]);
      return { research, named };
    } finally { await fx.runtime.close(); }
  };
  const first = await launch();
  const second = await launch();
  expect(second.research.request).not.toBe(first.research.request);
  expect(second.research.sessionFile).toBe(`/private/${second.research.request}.jsonl`);
  expect(first.named).toMatchObject({ id: "named", state: "done" });
  expect(second.named).toMatchObject({ id: "named", state: "done" });
  expect(typeof first.named?.sessionFile).toBe("string");
  expect(typeof second.named?.sessionFile).toBe("string");
  expect(second.named?.sessionFile).not.toBe(first.named?.sessionFile);
});

test("queued explore cancellation and close never launch and settle canonical not-started records", async () => {
  const three = Promise.withResolvers<void>();
  const fx = fixture((_call, signal) => { if (fx.calls.length === 3) three.resolve(); return cancelled(signal); });
  const controller = new AbortController();
  const requests = [0, 1, 2, 3, 4].map(id => fx.runtime.explore(String(id), "question", id === 3 ? controller.signal : undefined));
  try {
    await three.promise;
    controller.abort();
    const queued = await requests[3]!;
    expect(queued.outcome).toBe("uncertain");
    expect(queued.report).toMatchObject({ status: "failed", stopReason: "cancelled" });
    expect(fx.calls).toHaveLength(3);
    expect(fx.runtime.snapshot().children[3]?.state).toBe("cancelled");
    expect(queued.sessionFile).toBe(`/private/${queued.request}.jsonl`);
    expect(fx.records.filter(record => record.request === queued.request).map(record => [record.action, record.outcome])).toEqual([
      ["prepare", undefined], ["finish", "not_started"],
    ]);
    await fx.runtime.close();
    const closed = await requests[4]!;
    expect(closed.report).toMatchObject({ status: "failed", stopReason: "cancelled" });
    expect(fx.records.filter(record => record.request === closed.request).map(record => [record.action, record.outcome])).toEqual([
      ["prepare", undefined], ["finish", "not_started"],
    ]);
    expect(fx.calls).toHaveLength(3);
  } finally { await fx.runtime.close(); await Promise.all(requests); }
});

test("explore timeout cancels the runner and records uncertain spend exactly once", async () => {
  jest.useFakeTimers();
  const started = Promise.withResolvers<void>();
  const fx = fixture((_call, signal) => { started.resolve(); return cancelled(signal); }, { ...settings, timeout_secs: 1 });
  try {
    const request = fx.runtime.explore("timeout", "question");
    await started.promise;
    jest.advanceTimersByTime(1000);
    const result = await request;
    expect(result).toMatchObject({ outcome: "uncertain", report: { status: "failed", stopReason: "timeout" } });
    expect(fx.records.map(record => record.action)).toEqual(["prepare", "started", "finish"]);
    expect(fx.records.at(-1)?.outcome).toBe("uncertain");
  } finally { await fx.runtime.close(); }
});

test("workspace Explore model applies only to standalone calls on the configured provider", async () => {
  const { capabilities, guidance } = workspaceSettings("/unused", "worker", undefined, {
    explore: { model: "openai-codex/gpt-6-luna" },
  });
  const explore = auxiliarySettings("worker", capabilities.enabled, guidance.settings).explore;
  if (!explore) throw new Error("Explore is disabled");
  const answer = async (): Promise<ResearchResult> => ({ report: "answer" });
  const codex = fixture(answer, explore);
  const anthropic = fixture(answer, explore, "anthropic");
  try {
    await codex.runtime.explore("standalone", "research");
    await codex.runtime.start("task", [{ id: "named", kind: "explore", title: "Research", brief: "research", owns: [] }]);
    await codex.runtime.wait(["named"]);
    await anthropic.runtime.explore("standalone", "research");
    expect(codex.calls.map(call => call.model)).toMatchObject([
      { provider: "openai-codex", model: "gpt-6-luna" },
      { provider: "openai-codex", model: "normal" },
    ]);
    expect(anthropic.calls[0]?.model).toMatchObject({ provider: "anthropic", model: "normal" });
  } finally { await Promise.all([codex.runtime.close(), anthropic.runtime.close()]); }
});

test("Explore priority is validated and restricted to the Codex research runner", async () => {
  const explore: ExploreSettings = { ...settings, model: "gpt-6-luna", service_tier: "priority" };
  expect(ExploreSettingsSchema.parse(explore).service_tier).toBe("priority");
  expect(() => ExploreSettingsSchema.parse({ ...explore, service_tier: "fast" })).toThrow();
  const answer = async (): Promise<ResearchResult> => ({ report: "answer", researchReport: { status: "completed", stopReason: "completed" } });
  const priority = fixture(answer, explore);
  const other = fixture(answer, explore, "anthropic");
  const standard = fixture(answer, { ...explore, service_tier: "default" });
  try {
    await priority.runtime.explore("one", "research");
    expect(priority.calls[0]?.model).toMatchObject({ provider: "openai-codex", model: "gpt-6-luna" });
    expect(priority.calls[0]?.hooks.execution?.serviceTier).toBe("priority");
    await expect(other.runtime.explore("one", "research")).rejects.toThrow("requires the openai-codex provider");
    expect(other.calls).toHaveLength(0);
    expect(other.records).toHaveLength(0);
    await standard.runtime.explore("one", "standard research");
    expect(standard.calls[0]?.hooks.execution?.serviceTier).toBeUndefined();
  } finally { await Promise.all([priority.runtime.close(), other.runtime.close(), standard.runtime.close()]); }
});

test("research presets and exploreTool preserve ceilings, context, partial status, identity and truncation", async () => {
  expect(researchBudget({ effort: "quick" }, 12)).toEqual({ maxTurns: 4, thinkingLevel: "off" });
  expect(researchBudget({ effort: "quick" }, 2).maxTurns).toBe(2);
  expect(researchBudget({ effort: "deep" }, 12)).toEqual({ maxTurns: 12, thinkingLevel: "high" });
  const fx = fixture(async () => ({ report: "e".repeat(32769), truncated: true, researchReport: { status: "partial", stopReason: "turn_limit" } }));
  try {
    const tool = exploreTool(fx.runtime);
    const result = await tool.execute("docs", { prompt: "Verify API", profile: "docs", effort: "quick",
      starting_points: "https://example.com/docs", constraints: "Version 2 only" }, undefined, undefined, undefined as never);
    expect(fx.calls[0]?.spec.brief).toBe("Verify API");
    expect(fx.calls[0]?.model).toMatchObject({ model: "normal", provider: "openai-codex" });
    expect(fx.calls[0]?.hooks.execution).toMatchObject({ maxTurns: 4,
      research: { profile: "docs", effort: "quick", starting_points: "https://example.com/docs", constraints: "Version 2 only" } });
    expect(result.details).toMatchObject({ request: fx.records[0]?.request, truncated: true, report: { status: "partial" } });
    const text = result.content.filter(block => block.type === "text").map(block => block.text).join("\n");
    const identity = fx.records[0]?.request;
    expect(typeof identity).toBe("string");
    expect(text).toContain(String(identity));
    expect(text).toContain("partial");
    expect(text).toContain("turn_limit");
    expect(text).toContain(`/private/${identity}.jsonl`);
    expect(text).not.toContain("e".repeat(32769));
    await expect(tool.execute("oversized", { prompt: "p".repeat(32768), constraints: "x" }, undefined, undefined, undefined as never)).rejects.toThrow("total");
    expect(fx.calls).toHaveLength(1);
  } finally { await fx.runtime.close(); }
});
