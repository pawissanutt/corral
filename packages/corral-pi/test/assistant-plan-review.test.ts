import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { HostSession } from "../src/host/session.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { assistantPlanTool, assistantPlanReviewTool, ASSISTANT_WORKFLOW_OPS } from "../src/roles/assistant/planning.ts";
import { AuxiliaryRuntime, type AuxiliaryResult } from "../src/extensions/subagent/runtime.ts";
import { PlanReviews, type LocalReviewProjection } from "../src/host/plan-review.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { reviewSnapshots } from "../src/extensions/subagent/review-snapshot.ts";
import { fakeCodexTransport, requestBody } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const barrier = () => { let resolve!: () => void; const promise = new Promise<void>(r => { resolve = r; }); return { promise, resolve }; };
const expected = { workflow: "W1", state_revision: 5, authority_epoch: 1, plan_revision: 3 };
function record(dir: string, plan = "# Draft r3\n"): LocalReviewProjection {
  const context = JSON.stringify({ original_request: "Improve eviction", decisions: ["retain API"] });
  writeFileSync(join(dir, "plan.md"), plan); writeFileSync(join(dir, "context.json"), context);
  return { review: "LR1", workflow: "W1", revision: 3, digest: sha256Hex(plan), invocation: sha256Hex("W1/LR1"),
    lifecycle: "prepared", result_revision: 1, notice_delivered: false,
    plan: { blob: `blob:sha256:${sha256Hex(plan)}`, bytes: Buffer.byteLength(plan) }, snapshot_file: join(dir, "plan.md"),
    context: { blob: `blob:sha256:${sha256Hex(context)}`, bytes: Buffer.byteLength(context) }, context_file: join(dir, "context.json") };
}
// Transport projections only: real CAS/admission/receipt/terminal semantics are exercised in
// the sibling Rust disposable-store protocol tests, not reimplemented in this fake CLI.
const REVIEW_FIXTURE = `
import { renameSync } from "node:fs";
let localReview = {}, dirty = false;
const file = join(dir, "local-review.json"), w = planningFixtureRead().workflow;
let r = existsSync(file) ? JSON.parse(readFileSync(file,"utf8")) : null;
const p = envelope.payload;
if (envelope.op === "assistant_plan_review") {
  if (p.action === "start" && !r) { r = JSON.parse(readFileSync(join(dir,"requested-review.json"),"utf8")); dirty=true; }
  if (p.action === "read") localReview = p.review ? r : {count:r ? 1 : 0,reviews:r ? [r] : [],pending:r && !r.notice_delivered ? [r.review] : [],next_pending_after:null};
  else localReview = r;
}
if (envelope.op === "event" && p.kind === "auxiliary" && p.plan_review) {
  const e = p.review_event;
  dirty = p.action === "started" || ["claim","stage","settle","delivered"].includes(e?.action);
  if (e?.action === "claim") { r.lifecycle="queued"; r.result_revision++; localReview={...r,claimed:true}; }
  if (p.action === "started") r.lifecycle="running";
  if (e?.action === "stage") r.findings_text=(r.findings_text ?? "")+e.chunk;
  if (e?.action === "settle") { r.lifecycle=e.lifecycle; r.result_revision++; localReview=r; }
  if (e?.action === "recover") localReview=r;
  if (e?.action === "delivered") { r.notice_delivered=true; localReview=r; }
}
if (envelope.op === "assistant_plan" && p.action === "publish") {
  w.current_plan.revision=4; w.state_revision++; dirty=true;
  const target=join(dir,"snapshot.json"), tmp=target+"."+process.pid;
  writeFileSync(tmp,JSON.stringify({workflow:w})); renameSync(tmp,target);
}
if (r) {
  r.current_revision=w.current_plan.revision; r.freshness=r.current_revision===r.revision ? "current" : "stale";
  if (dirty) { const tmp=file+"."+process.pid; writeFileSync(tmp,JSON.stringify(r)); renameSync(tmp,file); }
}
`;

test("public host review returns before held inference; main research and r4 publication continue; exact restricted r3 result survives reload", async () => {
  const assets = buildFixtureInstall(), dir = mkdtempSync(join(tmpdir(), "corral-local-review-host-"));
  const originalFetch = globalThis.fetch, release = barrier(), held = barrier();
  let host: HostSession | undefined;
  try {
    const modules = await loadModules(assets.dir, "assistant"), corral = writeFakeCorral(dir), role = assistantRole(PACKAGE_ROOT);
    const plan = "# Draft r3\n" + "Precise café 🦀\n".repeat(2200), r = record(dir, plan);
    writeFileSync(join(dir, "requested-review.json"), JSON.stringify(r));
    corral.snapshot({ workflow: { version: 1, ...expected, phase: "planning", owner: { kind: "local" }, current_plan: { revision: 3 } } });
    writeFileSync(corral.path, readFileSync(corral.path, "utf8").replace("process.stdout.write(JSON.stringify({ version: 1,", REVIEW_FIXTURE + "\nprocess.stdout.write(JSON.stringify({ version: 1,")
      .replace("...ended, ...boundary,", "...ended, ...boundary, ...localReview,"));
    const parent = fakeCodexTransport(), child = fakeCodexTransport();
    const network = guardedFetch((input, init) => {
      if ((requestBody(init) as { model: string }).model === "gpt-6-luna") return child.send(input, init);
      if (!parent.requests) {
        const request = corral.calls().find(c => c.envelope.payload.kind === "owner_input")?.envelope.request_id;
        if (!request) throw new Error("genuine owner request was not journaled");
        parent.script([{ toolCalls: [{ callId: "review", name: "corral_assistant_plan_review", arguments: {
          action: "start", expected, input: `A-1/g1/i${request.split("-").at(-1)}` } }] }, { text: "Review is running; continue discussion." }]);
      }
      return parent.send(input, init);
    });
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "assistant" as const, assistant: "A-1", generation: 1 }, events = new EventLog(1);
    const client = new CorralClient({ corral: corral.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: dir, agentDir: join(dir, "agent"), model: "gpt-6.1-sol", client, events, network,
      capabilities: { catalog: 9, digest: "a".repeat(64), preset: "full", enabled: ["subagents"] }, ui: "plain",
      guidance: { projectText: false, settings: { subagents: { model: "gpt-6-luna", max_parallel: 2, max_turns: 12, timeout_secs: 30 } } },
      modules: { ...modules, role: { ...role, corralOps: [...role.corralOps, ...ASSISTANT_WORKFLOW_OPS],
        tools: context => [...role.tools(context), assistantPlanTool, assistantPlanReviewTool] } },
      sessionManager: sdk.SessionManager.create(dir, join(dir, "sessions")) });
    await host.session.bindExtensions({}); await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const bytes = Buffer.from(plan), offsets = [0];
    while (offsets.at(-1)! < bytes.length) { let end = Math.min(bytes.length, offsets.at(-1)! + 16384);
      while (end < bytes.length && (bytes[end]! & 0xc0) === 0x80) end--; offsets.push(end); }
    child.script([...offsets.slice(0, -1).map((offset, i) => ({ toolCalls: [
      { callId: `plan-${i}`, name: "read_review_snapshot", arguments: { document: "plan", offset } },
      ...(i === 0 ? [{ callId: "context", name: "read_review_snapshot", arguments: { document: "context", offset: 0 } }] : [])] })),
      { toolCalls: [{ callId: "write", name: "write", arguments: { path: "bad.txt", content: "bad" } },
        { callId: "recurse", name: "explore", arguments: { prompt: "implement" } }] },
      { chunks: ["Advisory: no blocking findings. Plan eviction; source.txt:1."], afterFirstChunk: async () => { held.resolve(); await release.promise; } },
      { text: "independent main research completed" }]);
    await host.session.prompt("Please independently review this draft now.", { source: "interactive" }); await held.promise;
    expect(host.session.getLastAssistantText()).toContain("Review is running");
    const r4 = "# Draft r4\nChanged eviction; no implementation consent.\n";
    parent.script([{ toolCalls: [{ callId: "research", name: "explore", arguments: { prompt: "Independent main-session research" } }] },
      { toolCalls: [{ callId: "stage-r4", name: "corral_assistant_plan", arguments: { action: "stage", expected, offset: 0, chunk: r4 } }] },
      { toolCalls: [{ callId: "publish-r4", name: "corral_assistant_plan", arguments: { action: "publish", expected, bytes: Buffer.byteLength(r4), digest: sha256Hex(r4), stage: "draft" } }] },
      { text: "r4 published while r3 review continues" }]);
    await host.session.prompt("Research and revise the eviction design.", { source: "interactive" }); await host.drained();
    expect(host.session.getLastAssistantText()).toContain("r4 published");
    const done = nextEvent(events, e => e.kind === "local_plan_review" && (e.review_event as { action?: string })?.action === "delivered");
    release.resolve(); await done; await host.reviewNoticed(); await host.drained(); await host.session.waitForIdle();
    const saved = () => JSON.parse(readFileSync(join(dir, "local-review.json"), "utf8"));
    expect(saved()).toMatchObject({ lifecycle: "completed", revision: 3, current_revision: 4, freshness: "stale", notice_delivered: true });
    expect(saved().findings_text).toContain("no blocking findings"); expect(existsSync(join(dir, "bad.txt"))).toBeFalse();
    const transcript = readFileSync(join(dir, `auxiliary-${r.invocation}.jsonl`), "utf8");
    expect(transcript).toContain("Tool write not found"); expect(transcript).toContain("Tool explore not found"); expect(transcript).toContain('"usage"');
    expect(readFileSync(r.snapshot_file, "utf8")).toBe(plan);
    const before = child.requests; await host.session.reload(); expect(child.requests).toBe(before);
    expect(corral.calls().filter(c => c.envelope.op === "assistant_plan_review" && c.envelope.payload.action === "start")).toHaveLength(1);
    expect(corral.calls().filter(c => c.envelope.payload.plan_review && c.envelope.payload.action === "prepare")).toHaveLength(1);
    expect(corral.calls().filter(c => c.envelope.payload.plan_review && c.envelope.payload.action === "finish").map(c => c.envelope.payload.outcome)).toEqual(["completed"]);
  } finally { release.resolve(); await host?.close(); globalThis.fetch = originalFetch; assets.cleanup(); rmSync(dir, { recursive: true, force: true }); }
});

test("unavailable review never starts a provider; recovery never replays prepared/queued/running work or terminal notices", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-recovery-"));
  try {
    const errors: unknown[] = []; let r = record(dir), starts = 0, notices = 0;
    const events: string[] = [];
    const controller = () => new PlanReviews({
      async call(_id, p) { if (p.action === "start") starts++; return p.action === "read" && !p.review ? { count: 1, pending: r.notice_delivered ? [] : [r.review], next_pending_after: null } : r; },
      async event(_r, event) {
        events.push(String(event.action));
        if (event.action === "settle") r = { ...r, lifecycle: String(event.lifecycle), result_revision: r.result_revision + 1 };
        if (event.action === "recover" && ["prepared", "queued", "running"].includes(r.lifecycle)) r = { ...r, lifecycle: "interrupted", result_revision: r.result_revision + 1 };
        if (event.action === "delivered") r = { ...r, notice_delivered: true }; return r;
      }, async notify() { notices++; return true; }, error: error => errors.push(error) });
    const unavailable = controller();
    expect((await unavailable.start("start", expected, "owner-request")).lifecycle).toBe("unavailable");
    expect(events).not.toContain("claim"); await unavailable.close();
    for (const phase of ["prepared", "queued", "running", "completed", "failed", "cancelled", "timed_out", "incomplete"]) {
      r = { ...record(dir), lifecycle: phase, result_revision: 4 }; const fresh = controller(), before = starts;
      await fresh.recover(); await fresh.recover(); await fresh.close(); expect(starts).toBe(before);
      expect(r.lifecycle).toBe(["prepared", "queued", "running"].includes(phase) ? "interrupted" : phase); expect(r.notice_delivered).toBeTrue();
    }
    expect(notices).toBe(8); expect(errors).toEqual([]);
    for (const count of [2, 4]) {
      let reads = 0;
      const history = new PlanReviews({ async call() { reads++; return { count, pending: [], next_pending_after: null }; },
        async event() { throw new Error("delivered history must not be replayed"); },
        async notify() { throw new Error("no pending notice"); }, error: e => errors.push(e) });
      await history.recover(); expect(reads).toBe(1); await history.close();
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("reload fences an in-flight start or claim acknowledgement without admitting a child", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-admission-"));
  try {
    for (const boundary of ["start", "claim"]) {
      let r = record(dir), executions = 0, prepares = 0;
      const held = barrier(), release = barrier();
      const runtime = new AuxiliaryRuntime({ generation: 1, sessionId: () => "parent", provider: "openai-codex", model: "fixture",
        settings: { explore: { model: null, max_parallel: 1, max_turns: 2, timeout_secs: 30 } },
        async record(p) { prepares++; return { session_id: p.request, session_file: join(dir, "never.jsonl") }; },
        async execute() { executions++; return { text: "never", outcome: "completed" }; } });
      const controller = new PlanReviews({ auxiliary: runtime,
        async call() { if (boundary === "start") { held.resolve(); await release.promise; } return r; },
        async event(_r, e) {
          if (e.action === "claim") { held.resolve(); await release.promise; r = { ...r, lifecycle: "queued" }; return { ...r, claimed: true }; }
          if (e.action === "recover") r = { ...r, lifecycle: "interrupted" }; return r;
        }, async notify() { return false; }, error: e => { throw e; } });
      const pending = controller.start("start", expected, "owner-request"); await held.promise;
      await controller.close(true); release.resolve();
      expect((await pending).lifecycle).toBe("interrupted"); expect(executions).toBe(0); expect(prepares).toBe(0);
      await controller.close(); await runtime.close();
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("review terminal labels retain auxiliary accounting on failure, timeout and unusable output", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-outcomes-"));
  try {
    const oversized = "x".repeat(1_048_577);
    const cases: { state: string; result: AuxiliaryResult | Error; truncated?: true }[] = [
      { state: "incomplete", result: { text: "\0\n", outcome: "completed", report: { status: "completed", stopReason: "completed" } } },
      { state: "timed_out", result: { text: "partial", outcome: "uncertain", report: { status: "partial", stopReason: "timeout" } } },
      { state: "failed", result: new Error("provider failure") }, { state: "unavailable", result: new Error("model_unavailable") },
      { state: "timed_out", result: { text: oversized, outcome: "uncertain", report: { status: "partial", stopReason: "timeout" } } },
      { state: "cancelled", result: { text: oversized, outcome: "uncertain", report: { status: "partial", stopReason: "cancelled" } } },
      { state: "failed", result: { text: oversized, outcome: "uncertain", report: { status: "partial", stopReason: "provider_error" } } },
      { state: "incomplete", result: { text: oversized, outcome: "completed", report: { status: "completed", stopReason: "completed" } } },
      { state: "timed_out", truncated: true, result: { text: "partial", outcome: "uncertain", report: { status: "partial", stopReason: "timeout" } } },
      { state: "cancelled", truncated: true, result: { text: "partial", outcome: "uncertain", report: { status: "partial", stopReason: "cancelled" } } },
      { state: "failed", truncated: true, result: { text: "partial", outcome: "uncertain", report: { status: "partial", stopReason: "provider_error" } } },
      { state: "incomplete", truncated: true, result: { text: "partial", outcome: "completed", report: { status: "completed", stopReason: "completed" } } },
    ];
    for (const item of cases) {
      let r = record(dir), executions = 0; const records: Record<string, unknown>[] = [];
      const runtime = new AuxiliaryRuntime({ generation: 1, sessionId: () => "parent", provider: "openai-codex", model: "fixture",
        settings: { explore: { model: null, max_parallel: 1, max_turns: 2, timeout_secs: 30 } },
        async record(p) { records.push(p); return { session_id: p.request, session_file: join(dir, "fixture.jsonl") }; },
        async execute() { executions++; if (item.result instanceof Error) throw item.result; return item.result; } });
      if (item.truncated) {
        const startReview = runtime.startReview.bind(runtime);
        runtime.startReview = (...args) => { const handle = startReview(...args);
          return { ...handle, done: handle.done.then(result => ({ ...result, truncated: true })) }; };
      }
      const controller = new PlanReviews({ auxiliary: runtime, async call() { return r; },
        async event(_r, e) { if (e.action === "claim") return { ...r, claimed: true };
          if (e.action === "settle") r = { ...r, lifecycle: String(e.lifecycle) }; return r; },
        async notify() { return true; }, error: e => { throw e; } });
      await controller.start("start", expected, "owner-request"); await controller.drained();
      expect(r.lifecycle).toBe(item.state); expect(executions).toBe(1);
      expect(records.filter(p => p.action === "finish").map(p => p.outcome)).toEqual([item.result instanceof Error ? "uncertain" : item.result.outcome]);
      await controller.start("duplicate", expected, "owner-request"); expect(executions).toBe(1);
      await controller.close(); await runtime.close();
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("snapshot reader rejects altered bytes and partial consumption cannot claim full coverage", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-snapshot-"));
  try {
    const r = record(dir, "🦀".repeat(20));
    const mission = { association: { workflow: r.workflow, review: r.review, digest: r.digest },
      plan: { path: r.snapshot_file, bytes: r.plan.bytes, digest: r.digest },
      context: { path: r.context_file, bytes: r.context.bytes, digest: r.context.blob.slice("blob:sha256:".length) } };
    const reader = reviewSnapshots(mission);
    await reader.tool.execute("partial", { document: "plan", offset: 0, limit: 4 }, undefined, undefined, {} as never);
    expect(reader.complete()).toBeFalse(); writeFileSync(r.snapshot_file, "x".repeat(r.plan.bytes));
    expect(() => reviewSnapshots(mission)).toThrow("digest differs");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
