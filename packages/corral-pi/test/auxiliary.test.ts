import { expect, test } from "bun:test";
import { AuxiliaryRuntime, type AuxiliaryResult, type Invocation } from "../src/extensions/subagent/runtime.ts";
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
  first.resolve();
  await four.promise;
  expect((await requests[0])?.generation).toBe(7);
  expect(fx.runtime.state().explore.running).toBe(3);
  await fx.runtime.close();
  await Promise.all(requests);
  expect(fx.records.filter((r) => r.action === "prepare")).toHaveLength(4);
  expect(fx.records.filter((r) => r.action === "finish" && r.outcome === "uncertain")).toHaveLength(3);
  expect(fx.runtime.state().explore).toEqual({ running: 0, queued: 0 });
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

test("child ceiling denies writes, shell, MCP, hooks, recursive spawn and Corral at every depth", () => {
  const parent = ["read", "grep", "find", "ls", "lsp_definition", "web_fetch", "write", "edit", "bash", "explore", "oracle", "codemode", "mcp_call", "corral_done"];
  const allowed = new Set(exploreLoadout(parent));
  expect([...allowed]).toEqual(parent.slice(0, 6));
  for (const nested of [false, true]) {
    expect(executionRefusal("read", nested, allowed, new Set())).toBeUndefined();
    for (const tool of parent.slice(6)) expect(executionRefusal(tool, nested, allowed, new Set())).toBeDefined();
  }
  expect(exploreLoadout(["read"])).toEqual(["read"]);
  expect(executionRefusal("corral_done", true, new Set(parent), new Set(["corral_done"]))).toContain("directly");
});
