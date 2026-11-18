import { afterEach, expect, jest, test } from "bun:test";
import type { AgentToolResult } from "@earendil-works/pi-coding-agent";
import { expertTool, exploreTool } from "./index.ts";
import type { HelperInspection } from "./inspection.ts";
import { AuxiliaryRuntime, type AuxiliaryKind, type AuxiliaryResult, type Invocation } from "./runtime.ts";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { FleetRuntime } from "../fleet/runtime.ts";

class WatchedRuntime extends AuxiliaryRuntime {
  subscriptions = 0;
  override watch(kind: AuxiliaryKind, id: string, changed: (inspection: HelperInspection) => void): () => void {
    const release = super.watch(kind, id, changed);
    this.subscriptions++;
    let subscribed = true;
    return () => {
      if (!subscribed) return;
      subscribed = false;
      this.subscriptions--;
      release();
    };
  }
}

class WatchedFleetRuntime extends FleetRuntime {
  subscriptions = 0;
  override watch(kind: "explore", id: string, changed: (inspection: HelperInspection) => void): () => void {
    const release = super.watch(kind, id, changed);
    this.subscriptions++;
    let subscribed = true;
    return () => {
      if (!subscribed) return;
      subscribed = false;
      this.subscriptions--;
      release();
    };
  }
}

afterEach(() => { jest.useRealTimers(); });

function fixture(kind: "explore" | "expert") {
  const started = Promise.withResolvers<Pick<Invocation, "observe">>();
  const child = Promise.withResolvers<AuxiliaryResult>();
  if (kind === "expert") {
    const runtime = new WatchedRuntime({ provider: "fixture", model: "parent", sessionId: () => "parent",
      settings: { expert: { model: "strong", max_parallel: 1, max_turns: 4, timeout_secs: 60 } },
      record: async payload => ({ session_id: payload.request, session_file: "/unused" }),
      execute: async invocation => { started.resolve(invocation); return child.promise; },
    });
    return { runtime, tool: expertTool(runtime), started: started.promise, child };
  }
  const runtime = new WatchedFleetRuntime({ sdk: {} as typeof Sdk, models: {} as ModelRuntime,
    approved: ["fixture"], parent: { provider: "fixture", model: "parent" }, agentDir: "/unused", localTools: [],
    context: { dir: "/unused", session: { worktree: "/repo", name: "test" }, settings: { maxParallel: 2, profiles: [] } },
    research: { settings: { model: "child", max_parallel: 2, max_turns: 4, timeout_secs: 60 } },
    record: async payload => ({ session_id: payload.request, session_file: "/unused" }),
    changed() {}, deliverToParent() {},
    runner: { run(_spec, _model, hooks) {
      started.resolve({ observe: hooks.onEvent });
      return { done: child.promise.then(result => ({ report: result.text, researchReport: result.report, turns: result.turns })),
        usage: () => ({ input: 0, output: 0, turns: 2 }), async steer() {}, async setModel() { return "applied"; } };
    } },
  });
  return { runtime, tool: exploreTool(runtime), started: started.promise, child };
}

for (const kind of ["explore", "expert"] as const) {
  test(`${kind} streams bounded child activity and cumulative tool counts, then unsubscribes without changing the answer`, async () => {
    jest.useFakeTimers();
    const fx = fixture(kind);
    const tool = fx.tool;
    const params = kind === "explore" ? { prompt: "Trace this" } : { question: "Trace this" };
    const updates: AgentToolResult<unknown>[] = [];
    try {
      const call = tool.execute("research", params, undefined, update => updates.push(update), undefined as never);
      const invocation = await fx.started;
      expect(fx.runtime.subscriptions).toBe(1);
      invocation.observe?.({ type: "assistant_start" });
      invocation.observe?.({ type: "tool", id: "one", name: "read", phase: "running" });
      expect(updates).toEqual([{ content: [{ type: "text", text: "read: running" }], details: { progress: { turns: 1, tools: 1 } } }]);
      jest.advanceTimersByTime(100);
      invocation.observe?.({ type: "tool", id: "one", name: "read", phase: "completed", text: "first result" });
      invocation.observe?.({ type: "assistant_start" });
      invocation.observe?.({ type: "tool", id: "two", name: "grep", phase: "running", text: `${"old".repeat(2000)}\n${"latest".repeat(100)}` });
      expect(updates).toHaveLength(1);
      jest.advanceTimersByTime(150);
      expect(updates.at(-1)?.details).toEqual({ progress: { turns: 2, tools: 2 } });
      const line = updates.at(-1)?.content[0];
      if (line?.type !== "text") throw new Error("activity text missing");
      expect(line.text.startsWith("grep: ")).toBe(true);
      expect(line.text.includes("latest")).toBe(true);
      expect(line.text.includes("old")).toBe(false);
      expect(line.text.length).toBeLessThanOrEqual(200);
      expect(line.text.includes("\n")).toBe(false);
      jest.advanceTimersByTime(500);
      expect(updates).toHaveLength(2);
      fx.child.resolve({ text: "unchanged answer", outcome: "completed", turns: 2 });
      const result = await call;
      expect(fx.runtime.subscriptions).toBe(0);
      expect(result).toEqual(await tool.execute("research", params, undefined, undefined, undefined as never));
      invocation.observe?.({ type: "assistant_delta", text: "late text" });
      jest.advanceTimersByTime(500);
      expect(updates).toHaveLength(2);
    } finally {
      fx.child.resolve({ text: "unchanged answer", outcome: "completed", turns: 2 });
      await fx.runtime.close();
    }
  });
}

test("explore abort immediately unsubscribes and cancels a pending progress update", async () => {
  jest.useFakeTimers();
  const fx = fixture("explore");
  const controller = new AbortController();
  const updates: AgentToolResult<unknown>[] = [];
  try {
    const call = fx.tool.execute("cancelled", { prompt: "Trace this" }, controller.signal,
      update => updates.push(update), undefined as never);
    const invocation = await fx.started;
    invocation.observe?.({ type: "tool", id: "one", name: "read", phase: "running" });
    invocation.observe?.({ type: "tool", id: "two", name: "grep", phase: "running" });
    expect(updates).toHaveLength(1);
    controller.abort();
    expect(fx.runtime.subscriptions).toBe(0);
    jest.advanceTimersByTime(500);
    expect(updates).toHaveLength(1);
    fx.child.resolve({ text: "partial answer", outcome: "uncertain", turns: 1 });
    await call;
    expect(fx.runtime.subscriptions).toBe(0);
  } finally {
    fx.child.resolve({ text: "partial answer", outcome: "uncertain", turns: 1 });
    await fx.runtime.close();
  }
});
