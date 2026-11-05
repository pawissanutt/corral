import { expect, test } from "bun:test";
import { AuxiliaryRuntime, type AuxiliaryResult, type Invocation } from "../src/extensions/subagent/runtime.ts";
import { ExploreSettingsSchema, OracleSettingsSchema } from "../src/extensions/subagent/settings.ts";
import { executionRefusal, exploreLoadout } from "../src/roles/common/execution.ts";

function barrier<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
const settings = { explore: { max_parallel: 3, model: null, max_turns: 12, timeout_secs: 30 },
  oracle: { model: "stronger", timeout_secs: 30 } };
function fixture(execute: (i: Invocation, s: AbortSignal) => Promise<AuxiliaryResult>) {
  const records: Record<string, unknown>[] = [];
  const runtime = new AuxiliaryRuntime({ settings, generation: 7, provider: "openai-codex", model: "normal",
    sessionId: () => "parent", execute,
    async record(payload) {
      records.push(payload);
      return { session_file: `/private/${payload.request}.jsonl`, session_id: payload.request };
    },
  });
  return { records, runtime };
}

test("four explore requests queue at three, preserve attribution and fence active/queued work", async () => {
  const three = barrier();
  const four = barrier();
  const first = barrier();
  const invocations: Invocation[] = [];
  const fx = fixture(async (i, signal) => {
    invocations.push(i);
    if (invocations.length === 3) three.resolve();
    if (invocations.length === 4) four.resolve();
    if (i.prompt === "0") await first.promise;
    else await new Promise<void>((resolve) => signal.addEventListener("abort", () => resolve(), { once: true }));
    return { text: i.prompt, outcome: signal.aborted ? "uncertain" : "completed" };
  });
  const requests = [0, 1, 2, 3].map((i) => fx.runtime.run("explore", String(i), String(i)));
  await three.promise;
  expect(fx.runtime.state().explore).toEqual({ running: 3, queued: 1 });
  expect(invocations).toHaveLength(3);
  expect(fx.runtime.inspection().map(child => child.state)).toEqual(["running", "running", "running", "queued"]);
  expect(fx.runtime.inspection().every(child => child.parent === "parent" && child.sessionFile?.endsWith(`${child.request}.jsonl`))).toBeTrue();
  first.resolve();
  await four.promise;
  expect((await requests[0])?.generation).toBe(7);
  expect(fx.runtime.state().explore.running).toBe(3);
  await fx.runtime.close();
  await Promise.all(requests);
  expect(fx.records.filter((r) => r.action === "prepare")).toHaveLength(4);
  expect(fx.records.filter((r) => r.action === "finish" && r.outcome === "uncertain")).toHaveLength(3);
  expect(fx.runtime.state().explore).toEqual({ running: 0, queued: 0 });
  expect(fx.runtime.inspection().map(child => child.state)).toEqual(["completed", "uncertain", "uncertain", "uncertain"]);
  await expect(fx.runtime.run("explore", "new", "no")).rejects.toThrow("fenced/closed");
  console.log("explore: 3 running + 1 queued; completion admits fourth; fencing cancels remaining children; generation=7");
});

test("queued cancellation never executes and close rejects queued children", async () => {
  const three = barrier();
  let count = 0;
  const fx = fixture(async (_i, signal) => {
    if (++count === 3) three.resolve();
    await new Promise<void>((resolve) => signal.addEventListener("abort", () => resolve(), { once: true }));
    return { text: "cancelled", outcome: "uncertain" };
  });
  const requests = [0, 1, 2, 3].map((i) => fx.runtime.run("explore", String(i), "question"));
  // Observe rejection immediately rather than leak a temporarily unhandled promise.
  const settled = Promise.allSettled(requests);
  await three.promise;
  await fx.runtime.close();
  expect((await settled)[3]?.status).toBe("rejected");
  expect(count).toBe(3);
  expect(fx.runtime.inspection()[3]?.state).toBe("not_started");
  expect(fx.records.filter((r) => r.outcome === "not_started")).toHaveLength(1);
});

test("oracle cancellation uses the shared attribution seam and records uncertain spend once", async () => {
  const response = barrier();
  let model = "";
  const fx = fixture(async (invocation, signal) => {
    model = invocation.model;
    expect(invocation.kind).toBe("oracle");
    expect(invocation.maxTurns).toBe(1);
    response.resolve();
    await new Promise<void>((resolve) => signal.addEventListener("abort", () => resolve(), { once: true }));
    return { text: "partial", outcome: "uncertain" };
  });
  const cancel = new AbortController();
  const request = fx.runtime.run("oracle", "call", "question", cancel.signal);
  expect(fx.runtime.run("oracle", "call", "question", cancel.signal)).toBe(request);
  await response.promise;
  cancel.abort();
  expect((await request).outcome).toBe("uncertain");
  expect(model).toBe("stronger");
  expect(fx.records.map((r) => r.action)).toEqual(["prepare", "started", "finish"]);
  expect(fx.records[2]?.outcome).toBe("uncertain");
  console.log("oracle: configured stronger model; response cancelled; one uncertain terminal record; duplicate request reused");
});

test("disabled/unconfigured auxiliary calls do not contact a provider or create billing records", async () => {
  let calls = 0;
  const runtime = new AuxiliaryRuntime({ settings: { oracle: { model: null, timeout_secs: 30 } },
    generation: 1, provider: "fixture", model: "parent", sessionId: () => "parent",
    record: async () => { calls++; return {}; }, execute: async () => { calls++; return { text: "bad", outcome: "completed" }; },
  });
  await expect(runtime.run("explore", "disabled", "question")).rejects.toThrow("disabled");
  await expect(runtime.run("oracle", "unconfigured", "question")).rejects.toThrow("not configured");
  expect(calls).toBe(0);
  await runtime.close();
});

test("Explore priority is validated and scoped without changing Oracle or other providers", async () => {
  const explore = { ...settings.explore, model: "gpt-6-luna", service_tier: "priority" as const };
  expect(ExploreSettingsSchema.parse(explore).service_tier).toBe("priority");
  expect(() => ExploreSettingsSchema.parse({ ...explore, service_tier: "fast" })).toThrow();
  expect(() => OracleSettingsSchema.parse({ ...settings.oracle, service_tier: "priority" })).toThrow();
  const invocations: Invocation[] = [];
  const options = { settings: { explore, oracle: settings.oracle }, generation: 1, provider: "openai-codex", model: "parent", sessionId: () => "parent",
    record: async (value: Record<string, unknown>) => ({ session_id: value.request, session_file: "/unused/reference" }),
    execute: async (invocation: Invocation): Promise<AuxiliaryResult> => { invocations.push(invocation); return { outcome: "completed", text: "answer" }; },
  };
  const runtime = new AuxiliaryRuntime(options);
  await runtime.run("explore", "one", "research"); await runtime.run("oracle", "two", "consult");
  expect(invocations.map(invocation => [invocation.kind, invocation.model, invocation.serviceTier])).toEqual([
    ["explore", "gpt-6-luna", "priority"], ["oracle", "stronger", undefined],
  ]);
  const other = new AuxiliaryRuntime({ ...options, provider: "anthropic" });
  await expect(other.run("explore", "one", "research")).rejects.toThrow("requires the openai-codex provider");
  expect(invocations).toHaveLength(2);
  const standard = new AuxiliaryRuntime({ ...options, settings: { explore: { ...explore, service_tier: "default" } } });
  await standard.run("explore", "one", "standard research"); expect(invocations.at(-1)?.serviceTier).toBeUndefined();
  await Promise.all([runtime.close(), other.close(), standard.close()]);
});

test("child ceiling denies writes, shell, MCP, hooks, recursive spawn and Corral at every depth", () => {
  const parent = ["read", "grep", "find", "ls", "lsp_definition", "web_fetch", "web_read", "write", "edit", "bash", "explore", "oracle", "codemode", "mcp_call", "corral_done"];
  const allowed = new Set(exploreLoadout(parent));
  expect([...allowed]).toEqual(parent.slice(0, 7));
  for (const nested of [false, true]) {
    expect(executionRefusal("read", nested, allowed)).toBeUndefined();
    for (const tool of parent.slice(7)) expect(executionRefusal(tool, nested, allowed)).toBeDefined();
  }
  expect(exploreLoadout(["read"])).toEqual(["read"]);
  // The child ceiling, not a nesting rule, keeps Corral out of every auxiliary depth.
  expect(executionRefusal("corral_done", true, allowed)).toContain("loadout");
});
