import { expect, test } from "bun:test";
import type { AgentToolCallOutcome } from "@earendil-works/pi-agent-core";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";

import { codemodeTool } from "../../capabilities/codemode.ts";
import type { CellReport } from "./report.ts";
import { CODE_CALL_ENTRY, CODE_CELL_ENTRY, CodeRuntime, type CodeClock, type Executor } from "./runtime.ts";

class ManualClock implements CodeClock {
  private time = 1000;
  private readonly timers = new Set<{ due: number; changed: () => void }>();
  now(): number { return this.time; }
  schedule(changed: () => void, delayMs: number): () => void {
    const timer = { due: this.time + delayMs, changed };
    this.timers.add(timer);
    return () => { this.timers.delete(timer); };
  }
  advance(ms: number): void {
    const end = this.time + ms;
    for (;;) {
      const next = [...this.timers].sort((a, b) => a.due - b.due)[0];
      if (!next || next.due > end) break;
      this.time = next.due;
      this.timers.delete(next);
      next.changed();
    }
    this.time = end;
  }
  get pending(): number { return this.timers.size; }
}

function outcome(n: number, failed = false): AgentToolCallOutcome {
  return { toolCall: { type: "toolCall", id: `parent/${n}`, name: "read", arguments: { path: `file-${n}` } },
    result: { content: [{ type: "text", text: failed ? "missing file" : `read file-${n}` }], details: {} },
    isError: failed, durationMs: 5 };
}

function harness() {
  const clock = new ManualClock();
  const journal: { customType: string; data: Record<string, unknown> }[] = [];
  const events: { kind: string; data: Record<string, unknown> }[] = [];
  const waiting = new Set<{ kind: string; match: (data: Record<string, unknown>) => boolean; resolve: (data: Record<string, unknown>) => void }>();
  const notices: string[] = [];
  const runtime = new CodeRuntime({
    clock,
    journal: (data, customType = CODE_CALL_ENTRY) => { journal.push({ customType, data: structuredClone(data) }); },
    emit: (kind, data) => {
      events.push({ kind, data });
      for (const waiter of [...waiting]) {
        if (waiter.kind !== kind || !waiter.match(data)) continue;
        waiting.delete(waiter);
        waiter.resolve(data);
      }
    },
    notify: async (_key, text) => { notices.push(text); return "delivered"; },
  });
  return { runtime, clock, journal, notices,
    event(kind: string, match: (data: Record<string, unknown>) => boolean): Promise<Record<string, unknown>> {
      const found = events.find(event => event.kind === kind && match(event.data));
      if (found) return Promise.resolve(found.data);
      const { promise, resolve } = Promise.withResolvers<Record<string, unknown>>();
      waiting.add({ kind, match, resolve });
      return promise;
    },
  };
}

test("research cells stay foreground and return their nested results", async () => {
  const fx = harness();
  const started = Promise.withResolvers<void>();
  const gate = Promise.withResolvers<AgentToolCallOutcome>();
  const executor: Executor = { executeTool: async () => { started.resolve(); return gate.promise; } };
  const tool = codemodeTool(fx.runtime, names => [...names], true);
  try {
    expect(tool.description).not.toContain("detaches");
    expect(JSON.stringify(tool.parameters)).not.toContain("wait_ms");
    let finished = false;
    const pending = tool.execute("research", { code: 'print(await tools.read({path:"file-1"}))', wait_ms: 0 },
      undefined, undefined, executor as ExtensionToolContext).then(result => { finished = true; return result; });
    await started.promise;
    await new Promise(resolve => setTimeout(resolve, 10));
    expect(finished).toBe(false);
    gate.resolve(outcome(1));
    expect(JSON.stringify((await pending).content)).toContain("read file-1");
  } finally { gate.resolve(outcome(1)); await fx.runtime.close(); }
});

test("parallel children expose ordered arguments and failures live and in final tool details", async () => {
  const fx = harness();
  const started = Promise.withResolvers<void>();
  const gates = Array.from({ length: 3 }, () => Promise.withResolvers<AgentToolCallOutcome>());
  let count = 0;
  const executor: Executor = { executeTool: async () => {
    const gate = gates[count++];
    if (count === 3) started.resolve();
    if (!gate) throw new Error("unexpected call");
    return gate.promise;
  } };
  const updates: CellReport[] = [];
  const tool = codemodeTool(fx.runtime, names => [...names]);
  try {
    const pending = tool.execute("parent", { code: 'await Promise.all([1,2,3].map(n => tools.read({path: "file-" + n})));' },
      undefined, partial => {
        const details = partial.details as { cells: CellReport[] };
        updates.push(structuredClone(details.cells[0]!));
      }, executor as ExtensionToolContext);
    await started.promise;
    expect(fx.runtime.snapshot("parent")?.status).toBe("running");
    expect(fx.runtime.snapshot("parent")?.children.map(child => JSON.parse(child.args ?? "null")))
      .toEqual([{ path: "file-1" }, { path: "file-2" }, { path: "file-3" }]);
    expect(fx.runtime.snapshot("parent")?.calls.running).toBe(3);
    fx.clock.advance(20);
    gates[1]!.resolve(outcome(2, true));
    await fx.event("code_call", data => data.phase === "settled" && data.call === "cell-1.2");
    const live = fx.runtime.snapshot("parent");
    expect(live?.children[1]).toMatchObject({ ok: false, code: "tool_error", message: "missing file", durationMs: 20,
      id: "parent/2", resultLines: 1, resultBytes: 12 });
    // A fast rejection must not erase or freeze siblings when the cell itself settles first.
    fx.clock.advance(5);
    const first = outcome(1);
    first.result.content = [{ type: "text", text: `${"row é\n".repeat(20)}last` }];
    gates[0]!.resolve(first);
    gates[2]!.resolve(outcome(3));
    const final = await pending;
    const details = final.details as { cells: CellReport[] };
    const report = details.cells[0]!;
    expect(report.children).toHaveLength(3);
    expect(report.children.map(child => child.ok)).toEqual([true, false, true]);
    expect(report.children.map(child => child.durationMs)).toEqual([25, 20, 25]);
    expect(report.children[0]).toMatchObject({ resultLines: 21, resultBytes: 144 });
    expect(report.children[0]?.preview?.split("\n")).toHaveLength(12);
    expect(report.calls).toMatchObject({ total: 3, ok: 2, failed: 1, running: 0 });
    expect(report.children[1]).toMatchObject({ code: "tool_error", message: "missing file" });
    expect(updates.length).toBeGreaterThan(0);
    expect(updates[0]?.children[0]?.args).toBe('{"path":"file-1"}');
    expect(fx.clock.pending).toBe(0); // Foreground forwarding unsubscribes when execute returns.
  } finally {
    for (let n = 0; n < gates.length; n++) gates[n]!.resolve(outcome(n + 1));
    await fx.runtime.close();
  }
});

test("detached cells retain late calls, settle live, release watchers and restore their final journal state", async () => {
  const fx = harness();
  const first = Promise.withResolvers<AgentToolCallOutcome>();
  const later = Promise.withResolvers<AgentToolCallOutcome>();
  const startedLater = Promise.withResolvers<void>();
  let calls = 0;
  const executor: Executor = { executeTool: async () => {
    if (++calls === 1) return first.promise;
    startedLater.resolve();
    return later.promise;
  } };
  const seen: { at: number; status: string | undefined }[] = [];
  fx.runtime.watch("parent", () => { seen.push({ at: fx.clock.now(), status: fx.runtime.snapshot("parent")?.status }); });
  let resumed: CodeRuntime | undefined;
  try {
    const detached = await fx.runtime.run("parent", `
      await tools.read({path:"first"});
      const reply = await tools.read({path:"later"});
      print("notice-only");
      return reply.next[0] === "read another file";
    `, executor, undefined, 0);
    expect(detached.status).toBe("detached");
    fx.clock.advance(10);
    first.resolve(outcome(1));
    await startedLater.promise;
    expect(fx.runtime.snapshot("parent")?.children[1]).toMatchObject({ args: '{"path":"later"}', late: true, startedAt: 1010 });
    fx.clock.advance(90);
    expect(seen.at(-1)?.status).toBe("detached");
    fx.clock.advance(10);
    const refused = outcome(2, true);
    const refusal = { reason: "missing file", next: ["read another file"] };
    later.resolve({ ...refused, result: { ...refused.result, content: [], structuredContent: refusal } });
    await fx.event("code_cell", data => data.phase === "settled");
    const final = fx.runtime.snapshot("parent");
    expect(final).toMatchObject({ status: "ok", durationMs: 110, calls: { total: 2, ok: 1, failed: 1, running: 0 } });
    expect(final?.children[1]).toMatchObject({ structured: refusal, preview: JSON.stringify(refusal),
      ok: false, code: "tool_error", effect: "unknown" });
    expect(await fx.event("code_call", data => data.phase === "settled" && data.call === "cell-1.2"))
      .toMatchObject({ structured: refusal, ok: false });
    expect(final?.children.every(child => child.late && child.durationMs !== undefined)).toBe(true);
    fx.clock.advance(90);
    expect(seen.at(-1)?.status).toBe("ok");
    expect(seen.every((item, i) => i === 0 || item.at - seen[i - 1]!.at >= 100)).toBe(true);
    expect(fx.clock.pending).toBe(0);
    const notifications = seen.length;
    await fx.runtime.run("parent", 'await tools.read({path:"next"});', { executeTool: async () => outcome(3) }, undefined);
    fx.clock.advance(100);
    expect(seen).toHaveLength(notifications); // The settled cell's watcher is gone without an explicit release.
    const entries = fx.journal.filter(entry => entry.customType === CODE_CELL_ENTRY);
    expect(entries).toHaveLength(1);
    expect(entries[0]?.data).toMatchObject({ phase: "settled", tool_call_id: "parent", status: "ok", duration_ms: 110 });
    const saved = entries[0]?.data.report as Record<string, unknown>;
    expect("outputs" in saved).toBe(false);
    expect("value" in saved).toBe(false);
    resumed = new CodeRuntime({ history: () => entries.map(entry => entry.data),
      emit: () => {}, journal: () => {}, notify: async () => "delivered" });
    expect(resumed.snapshot("parent")).toMatchObject({ status: "ok", durationMs: 110, calls: { total: 2, running: 0 } });
    expect(resumed.snapshot("parent")?.children[1]).toMatchObject({ late: true, ok: false, structured: refusal });
    await fx.runtime.notified();
    expect(fx.notices[0]).toContain("notice-only");
    expect(fx.notices[0]).toContain("Return value: true");
    expect(fx.notices[0]).toContain(JSON.stringify(refusal));
  } finally {
    first.resolve(outcome(1));
    later.resolve(outcome(2));
    await fx.runtime.close();
    await resumed?.close();
  }
});

test("children that finish after their settled cell update retained and resumed counts and late flags", async () => {
  const fx = harness();
  const gates = Array.from({ length: 3 }, () => Promise.withResolvers<AgentToolCallOutcome>());
  const started = Promise.withResolvers<void>();
  let count = 0;
  const executor: Executor = { executeTool: async () => {
    const gate = gates[count++];
    if (count === 3) started.resolve();
    if (!gate) throw new Error("unexpected call");
    return gate.promise; // Deliberately ignores abort, like a tool that is already committing.
  } };
  let resumed: CodeRuntime | undefined;
  try {
    await fx.runtime.run("parent", 'await Promise.all([tools.read({n:1}), tools.read({n:2}), tools.corral_expert({question:"Where?"})]);', executor, undefined, 0);
    await started.promise;
    gates[0]!.resolve(outcome(1, true));
    await fx.event("code_cell", data => data.phase === "settled");
    const report = fx.runtime.snapshot("parent")!;
    expect(report.status).toBe("error");
    expect(report.calls.running).toBe(2);
    fx.clock.advance(30);
    const second = outcome(2);
    const detail = { state: "file", range: { start: 1, end: 3 } };
    gates[1]!.resolve({ ...second, result: { ...second.result, structuredContent: detail } });
    const third = outcome(3);
    const conversationId = "c01275e4-6e53-4ec2-99cd-483d8fb9b5b3";
    gates[2]!.resolve({ ...third, toolCall: { ...third.toolCall, name: "corral_expert" }, result: { ...third.result, details: { conversationId } } });
    expect(await fx.event("code_call", data => data.phase === "settled" && data.call === "cell-1.3"))
      .toMatchObject({ conversation_id: conversationId });
    expect(fx.runtime.snapshot("parent")?.calls).toMatchObject({ total: 3, ok: 2, failed: 1, running: 0 });
    expect(fx.runtime.snapshot("parent")?.children.slice(1).every(child => child.ok && child.late && child.durationMs === 30)).toBe(true);
    expect(report.calls.running).toBe(0);
    resumed = new CodeRuntime({ history: () => fx.journal.map(entry => entry.data),
      emit: () => {}, journal: () => {}, notify: async () => "delivered" });
    expect(resumed.snapshot("parent")?.calls).toMatchObject({ total: 3, ok: 2, failed: 1, running: 0 });
    expect(resumed.snapshot("parent")?.children.slice(1).every(child => child.ok && child.late && child.durationMs === 30)).toBe(true);
    expect(resumed.snapshot("parent")?.children[1]).toMatchObject({ resultLines: 1, resultBytes: 11, structured: detail });
    expect(resumed.snapshot("parent")?.children[1]?.effect).toBeUndefined();
    expect(resumed.snapshot("parent")?.children[2]).toMatchObject({ conversationId });
  } finally {
    gates.forEach((gate, n) => gate.resolve(outcome(n + 1)));
    await fx.runtime.close();
    await resumed?.close();
  }
});

test("a 50-call cell coalesces child notifications under an injected clock without sleeps", async () => {
  const fx = harness();
  const notified: number[] = [];
  fx.runtime.watch("parent", () => { notified.push(fx.clock.now()); });
  let n = 0;
  try {
    const report = await fx.runtime.run("parent", 'for(let n=0;n<50;n++) await tools.read({path:"file",content:"x".repeat(3000)});', {
      executeTool: async () => {
        fx.clock.advance(1);
        return outcome(++n);
      },
    }, undefined);
    expect(report.children).toHaveLength(50);
    expect(notified).toHaveLength(1);
    fx.clock.advance(100);
    expect(notified).toHaveLength(2);
    expect(notified[1]! - notified[0]!).toBeGreaterThanOrEqual(100);
    expect(fx.clock.pending).toBe(0);
  } finally {
    await fx.runtime.close();
  }
});
