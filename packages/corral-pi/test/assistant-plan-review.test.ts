import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import auth, { fixtureTransport } from "./fixtures/auth/index.ts";
import { createModelCatalog } from "../src/host/catalog.ts";
import { authContext, restrictProvider } from "../src/host/session.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { AuxiliaryRuntime } from "../src/extensions/subagent/runtime.ts";
import { runAuxiliarySession } from "../src/extensions/subagent/session.ts";
import { PlanReviews, type LocalReviewProjection } from "../src/host/plan-review.ts";
import { sha256Hex } from "../src/tools/corral.ts";
import { reviewSnapshots } from "../src/extensions/subagent/review-snapshot.ts";

const barrier = () => { let resolve!: () => void; const promise = new Promise<void>(r => { resolve = r; }); return { promise, resolve }; };
const expected = { workflow: "W1", state_revision: 5, authority_epoch: 1, plan_revision: 3 };
function record(dir: string, plan = "# Draft r3\n") : LocalReviewProjection {
  const context = JSON.stringify({ original_request: "Improve eviction", decisions: ["retain API"] });
  writeFileSync(join(dir, "plan.md"), plan); writeFileSync(join(dir, "context.json"), context);
  return { review: "LR1", workflow: "W1", revision: 3, digest: sha256Hex(plan), invocation: sha256Hex("W1/LR1"),
    lifecycle: "prepared", result_revision: 1, notice_delivered: false,
    plan: { blob: `blob:sha256:${sha256Hex(plan)}`, bytes: Buffer.byteLength(plan) }, snapshot_file: join(dir, "plan.md"),
    context: { blob: `blob:sha256:${sha256Hex(context)}`, bytes: Buffer.byteLength(context) }, context_file: join(dir, "context.json") };
}

test("exact r3 review remains asynchronous through main research/r4, has restricted real SDK context and one terminal notice", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-local-review-"));
  const originalFetch = globalThis.fetch;
  let runtime: AuxiliaryRuntime | undefined;
  let controller: PlanReviews | undefined;
  const release = barrier();
  try {
    const transport = fixtureTransport();
    const network = guardedFetch(transport.send); globalThis.fetch = network.fetch;
    const models = await createModelCatalog(auth.credentials(authContext()), { providers: ["openai-codex"] });
    restrictProvider(models, "openai-codex", network.fetch as typeof globalThis.fetch);
    const plan = "# Draft r3\n" + "Precise café 🦀\n".repeat(2200);
    let r = record(dir, plan);
    const notices: string[] = [], errors: unknown[] = [], records: Record<string, unknown>[] = [], events: Record<string, unknown>[] = [];
    const settled = barrier(), held = barrier();
    let current = 3, starts = 0;
    const facts = () => ({ ...r, current_revision: current, freshness: current === 3 ? "current" : "stale" });
    runtime = new AuxiliaryRuntime({ settings: { explore: { model: null, max_parallel: 2, max_turns: 12, timeout_secs: 30 } },
      generation: 1, provider: "openai-codex", model: "gpt-6.1-sol", sessionId: () => "main-session",
      async record(payload) {
        records.push(payload);
        return { session_id: payload.request, session_file: join(dir, `${payload.request}.jsonl`) };
      },
      execute: (invocation, signal) => runAuxiliarySession({ sdk, models, provider: "openai-codex", cwd: dir, agentDir: join(dir, "agent"),
        parentLoadout: ["read", "grep", "find", "ls", "write", "bash", "explore"], localTools: [] }, invocation, signal),
    });
    controller = new PlanReviews({ auxiliary: runtime,
      async call(_id, payload) {
        if (payload.action === "start") { starts++; return facts(); }
        return payload.review ? facts() : { count: 1, reviews: [facts()] };
      },
      async event(_review, event) {
        events.push(event);
        if (event.action === "claim") { r = { ...r, lifecycle: "queued", result_revision: 2 }; return { ...facts(), claimed: true }; }
        if (event.action === "settle") { r = { ...r, lifecycle: String(event.lifecycle), result_revision: 3 }; }
        if (event.action === "delivered") { r = { ...r, notice_delivered: true }; settled.resolve(); }
        return facts();
      },
      async notify(key, text) { notices.push(`${key}: ${text}`); return true; }, error: error => { errors.push(error); settled.resolve(); },
    });
    // Fixed snapshot reader uses byte offsets; derive exact UTF-8 boundaries for the script.
    const bytes = Buffer.from(plan), offsets = [0];
    while (offsets.at(-1)! < bytes.length) {
      let end = Math.min(bytes.length, offsets.at(-1)! + 16384);
      while (end < bytes.length && (bytes[end]! & 0xc0) === 0x80) end--;
      offsets.push(end);
    }
    transport.script([
      ...offsets.slice(0, -1).map((offset, i) => ({ toolCalls: [
        { callId: `plan-${i}`, name: "read_review_snapshot", arguments: { document: "plan", offset } },
        ...(i === 0 ? [{ callId: "context", name: "read_review_snapshot", arguments: { document: "context", offset: 0 } }] : []),
      ] })),
      { toolCalls: [{ callId: "forbidden-write", name: "write", arguments: { path: "bad.txt", content: "bad" } },
        { callId: "forbidden-child", name: "explore", arguments: { prompt: "implement" } }] },
      { chunks: ["Advisory: no blocking findings. Plan section eviction; source.txt:1. "], afterFirstChunk: async () => { held.resolve(); await release.promise; } },
      { text: "main research completed" },
    ]);
    const started = await controller.start("request-review", expected, "owner-request-7");
    expect(started.lifecycle).toBe("queued"); // start has returned before child completion
    await held.promise;
    expect((await controller.start("duplicate-review", expected, "owner-request-7")).review).toBe("LR1");
    expect(starts).toBe(1);
    const research = await runtime.run("explore", "main-research", "Continue independent main-session research");
    expect(research.text).toBe("main research completed");
    current = 4; // A separately published canonical r4; the child still holds immutable r3.
    release.resolve(); await settled.promise;
    expect(errors).toEqual([]);
    expect(r.lifecycle).toBe("completed");
    expect((await controller.read("read", "LR1") as Record<string, unknown>).freshness).toBe("stale");
    expect(events.filter(e => e.action === "settle")).toMatchObject([{ lifecycle: "completed" }]);
    const findings = events.filter(e => e.action === "stage").map(e => e.chunk).join("");
    expect(findings).toContain("no blocking findings");
    expect(notices).toHaveLength(1); expect(notices[0]).toContain("reviewed r3");
    expect(records.filter(e => e.plan_review && e.action === "prepare")).toHaveLength(1);
    expect(records.filter(e => e.plan_review && e.action === "finish")).toMatchObject([{ outcome: "completed" }]);
    const child = readFileSync(join(dir, `${r.invocation}.jsonl`), "utf8");
    expect(child).toContain("Tool write not found"); expect(child).toContain("Tool explore not found");
    expect(child).toContain("Precise café 🦀"); expect(child).toContain('"usage"');
    expect(existsSync(join(dir, "bad.txt"))).toBeFalse();
    expect(readFileSync(r.snapshot_file, "utf8")).toBe(plan);
    const before = transport.requests;
    await controller.recover(); await controller.recover();
    expect(transport.requests).toBe(before); expect(notices).toHaveLength(1);
  } finally {
    release.resolve(); await controller?.close(); await runtime?.close();
    globalThis.fetch = originalFetch; rmSync(dir, { recursive: true, force: true });
  }
});

test("unavailable review never starts a provider; prepared/queued/running recovery never replays and terminal notice is retained", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-recovery-"));
  try {
    const errors: unknown[] = [];
    let r = record(dir), starts = 0, notices = 0;
    const events: string[] = [];
    const controller = () => new PlanReviews({
      async call(_id, p) { if (p.action === "start") starts++; return p.action === "read" && !p.review ? { count: 1 } : r; },
      async event(_r, event) {
        events.push(String(event.action));
        if (event.action === "settle") r = { ...r, lifecycle: String(event.lifecycle), result_revision: r.result_revision + 1 };
        if (event.action === "recover" && ["prepared", "queued", "running"].includes(r.lifecycle)) r = { ...r, lifecycle: "interrupted", result_revision: r.result_revision + 1 };
        if (event.action === "delivered") r = { ...r, notice_delivered: true };
        return r;
      },
      async notify() { notices++; return true; }, error: error => errors.push(error),
    });
    const unavailable = controller();
    expect((await unavailable.start("start", expected, "owner-request")).lifecycle).toBe("unavailable");
    expect(events).not.toContain("claim"); await unavailable.close();
    for (const phase of ["prepared", "queued", "running", "completed", "failed", "cancelled", "timed_out", "incomplete"]) {
      r = { ...record(dir), lifecycle: phase, result_revision: 4 };
      const fresh = controller();
      const before = starts;
      await fresh.recover(); await fresh.recover(); await fresh.close();
      expect(starts).toBe(before);
      expect(r.lifecycle).toBe(["prepared", "queued", "running"].includes(phase) ? "interrupted" : phase);
      expect(r.notice_delivered).toBeTrue();
    }
    expect(notices).toBe(8); expect(errors).toEqual([]);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("snapshot reader rejects altered bytes and cannot claim full coverage from a partial read", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-snapshot-"));
  try {
    const r = record(dir, "🦀".repeat(20));
    const mission = { association: { workflow: r.workflow, review: r.review, digest: r.digest },
      plan: { path: r.snapshot_file, bytes: r.plan.bytes, digest: r.digest },
      context: { path: r.context_file, bytes: r.context.bytes, digest: r.context.blob.slice("blob:sha256:".length) } };
    const reader = reviewSnapshots(mission);
    const execute = reader.tool.execute;
    await execute("partial", { document: "plan", offset: 0, limit: 4 }, undefined, undefined, {} as never);
    expect(reader.complete()).toBeFalse();
    writeFileSync(r.snapshot_file, "x".repeat(r.plan.bytes));
    expect(() => reviewSnapshots(mission)).toThrow("digest differs");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
