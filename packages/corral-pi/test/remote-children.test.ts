import { expect, test } from "bun:test";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { SESSION_PART_MAX } from "../src/bridge/protocol.ts";
import type { ChildSnapshot } from "../src/build/types.ts";
import { FleetRuntime } from "../src/extensions/fleet/runtime.ts";
import { childDetail, registerChildren } from "../src/host/remote/children.ts";
import { RemoteRegistry } from "../src/host/remote/registry.ts";
import { usagePart } from "../src/host/remote/usage.ts";

const child: ChildSnapshot = { id: "a", kind: "review", title: "Review", call: "review-1", brief: "Review the change",
  owns: [], after: [], profile: "default", model: "approved/parent", state: "done", createdAt: 1730000000000,
  edits: [], usage: { input: 4, output: 8, turns: 1 }, steers: [], pulse: [], history: "d", messages: [] };

test("a child detail clips the brief and the report and says so", () => {
  const brief = "設計\0".repeat(6_000), report = "🔎\n".repeat(9_000);
  const detail = childDetail({ ...child, brief, report, verdict: { verdict: "changes", summary: "Review summary",
    findings: [{ severity: "major", path: "src/lib.rs", line: 42, text: "Fix the update race" }], at: child.createdAt } }, true);
  expect(detail.brief_more).toBeTrue();
  expect(detail.report_more).toBeTrue();
  expect(brief.startsWith(detail.brief)).toBeTrue();
  expect(report.startsWith(detail.report!)).toBeTrue();
  expect(Buffer.byteLength(detail.brief)).toBeLessThanOrEqual(8 * 1024);
  expect(Buffer.byteLength(detail.report!)).toBeLessThanOrEqual(16 * 1024);
  expect(Buffer.byteLength(JSON.stringify(detail))).toBeLessThanOrEqual(SESSION_PART_MAX);
  expect(detail.verdict?.findings[0]?.line).toBe(42);
  expect(detail.transcript).toBeTrue();
});

test("set_model is refused for a model the fleet has not approved", async () => {
  const switched: string[] = [];
  const settings = { maxParallel: 1, exitPolicy: "none", profiles: [] } as const;
  const fleet = new FleetRuntime({ sdk, models: { getModels: () => [
    { provider: "approved", id: "parent" }, { provider: "approved", id: "other" }, { provider: "unapproved", id: "other" },
  ] } as unknown as ModelRuntime, approved: ["approved"], parent: { provider: "approved", model: "parent" },
    context: { dir: "/unused", settings, session: { worktree: "/unused", name: "test" } }, agentDir: "/unused", localTools: [],
    changed() {}, deliverToParent() {}, runner: { run(_spec, _model, _hooks, signal) {
      const done = Promise.withResolvers<{ report: string }>();
      signal.addEventListener("abort", () => done.resolve({ report: "" }), { once: true });
      return { done: done.promise, usage: () => ({ input: 0, output: 0, turns: 0 }), async steer() {},
        async setModel(provider, model) { switched.push(`${provider}/${model}`); return "applied"; } };
    } } });
  const register = new RemoteRegistry();
  registerChildren(register, { snapshot: () => fleet.snapshot(), fleet: () => fleet, ready: () => true, helpers: () => [] });
  const work = fleet.start("call", [{ id: child.id, title: child.title, brief: child.brief, owns: [] }]);
  try {
    const act = register.get("child_act")!;
    expect(await act({ child: child.id, action: "set_model", model: "unapproved/other" })).toMatchObject({ ok: false });
    expect(await act({ child: child.id, action: "set_model", model: "approved/missing" })).toMatchObject({ ok: false });
    expect(switched).toEqual([]);
    expect(fleet.snapshot().children[0]?.model).toBe("approved/parent");
    expect(await act({ child: child.id, action: "set_model", model: "approved/other" })).toEqual({ ok: true });
    expect(switched).toEqual(["approved/other"]);
  } finally { await fleet.close(); await work; }
});

test("an unpriced child leaves the usage total partial and not zero-priced", () => {
  const part = usagePart({ tokens: 0, cost_usd: 0, cost_partial: false, input: 0, output: 0,
    context_tokens: 0, context_window: 0, turns: 0 }, [child]);
  expect(part.rows.map(row => row.key)).toEqual(["pane", "child:a"]);
  expect(part.rows[1]?.cost_usd).toBeUndefined();
  expect(part.total).toEqual({ tokens: 12, cost_usd: undefined, partial: true });
});
