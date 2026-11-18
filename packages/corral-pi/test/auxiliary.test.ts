import { expect, test } from "bun:test";
import { AuxiliaryRuntime, type AuxiliaryResult, type Invocation } from "../src/extensions/subagent/runtime.ts";
import { executionRefusal, exploreLoadout } from "../src/roles/common/execution.ts";
import { researchResult } from "../src/extensions/subagent/research.ts";
import type { AssistantMessage } from "@earendil-works/pi-ai";

function barrier<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
const settings = { explore: { max_parallel: 3, model: null, max_turns: 12, timeout_secs: 30, service_tier: "priority" as const },
  oracle: { model: "stronger", timeout_secs: 30 } };
function fixture(execute: (i: Invocation, s: AbortSignal) => Promise<AuxiliaryResult>) {
  const records: Record<string, unknown>[] = [];
  const runtime = new AuxiliaryRuntime({ settings, provider: "openai-codex", model: "normal",
    sessionId: () => "parent", execute,
    async record(payload) {
      records.push(payload);
      return { session_file: `/private/${payload.request}.jsonl`, session_id: payload.request };
    },
  });
  return { records, runtime };
}

test("oracle cancellation uses the shared attribution seam and records uncertain spend once", async () => {
  const response = barrier();
  let model = "";
  const fx = fixture(async (invocation, signal) => {
    model = invocation.model;
    expect(invocation.kind).toBe("oracle");
    expect(invocation.maxTurns).toBe(1);
    response.resolve();
    expect(invocation.serviceTier).toBeUndefined();
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

test("relaunching the same parent with a reused tool call ID allocates a new auxiliary transcript", async () => {
  const execute = async (invocation: Invocation): Promise<AuxiliaryResult> => ({
    text: invocation.sessionId, outcome: "completed",
  });
  const first = fixture(execute);
  const before = await first.runtime.run("oracle", "call", "question");
  await first.runtime.close();
  const next = fixture(execute);
  try {
    const after = await next.runtime.run("oracle", "call", "question");
    expect(after.request).not.toBe(before.request);
    expect(after.sessionFile).toBe(`/private/${after.request}.jsonl`);
    expect(after.text).toBe(after.request);
  } finally { await next.runtime.close(); }
});

test("disabled/unconfigured auxiliary calls and bypassed explore calls create no billing records", async () => {
  let calls = 0;
  const runtime = new AuxiliaryRuntime({ settings: { oracle: { model: null, timeout_secs: 30 } },
    provider: "fixture", model: "parent", sessionId: () => "parent",
    record: async () => { calls++; return {}; }, execute: async () => { calls++; return { text: "bad", outcome: "completed" }; },
  });
  // @ts-expect-error Public auxiliary calls cannot enter the private review explore pool.
  await expect(runtime.run("explore", "bypass", "question")).rejects.toThrow("FleetRuntime.explore");
  await expect(runtime.run("expert", "disabled", "question")).rejects.toThrow("disabled");
  await expect(runtime.run("oracle", "unconfigured", "question")).rejects.toThrow("not configured");
  expect(calls).toBe(0);
  await runtime.close();
});

test("empty and token-limited research cannot masquerade as completed findings", () => {
  expect(researchResult([], "completed").report).toEqual({ status: "failed", stopReason: "empty_response" });
  const cutOff: AssistantMessage = { role: "assistant", api: "openai-responses", provider: "fixture", model: "fixture", timestamp: 1,
    content: [{ type: "thinking", thinking: "PRIVATE_REASONING" }, { type: "text", text: "source.ts:3 shows a partial" }],
    stopReason: "length", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
  const result = researchResult([cutOff], "completed");
  expect(result.report).toEqual({ status: "partial", stopReason: "output_limit" });
  expect(result.text).toContain("source.ts:3");
  expect(result.text).not.toContain("PRIVATE_REASONING");
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
