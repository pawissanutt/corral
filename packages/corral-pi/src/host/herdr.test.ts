// The shared herdr adapter (T-605), driven through a gated fake herdr: each test holds a call in
// flight to observe exactly what is reported, in which order and with which sequence numbers.
import { expect, test } from "bun:test";

import { EventLog } from "../bridge/events.ts";
import type { HostIdentity } from "../roles/types.ts";
import {
  AGENT, HerdrLifecycle, SEQ_SHIFT, SOURCE, TOKENS, answeredKeys, herdrLifecycle, herdrTarget,
} from "./herdr.ts";
import { nextEvent } from "../../test/support/wait.ts";

const EPOCH = 1_793_000_000_123;
const BASE = BigInt(EPOCH) << SEQ_SHIFT;
const WORKER: HostIdentity = { store: "st-1", project: "app", role: "worker", task: "T-7", run: "R2", generation: 3 };

/** A fake herdr whose every call waits until the test lets it finish. */
class GatedHerdr {
  readonly calls: string[][] = [];
  private readonly waiting: { resolve: () => void; reject: (error: Error) => void }[] = [];
  private arrived: (() => void) | undefined;
  failNext = false;

  readonly run = (bin: string, args: readonly string[]) => {
    expect(bin).toBe("/opt/herdr");
    this.calls.push([...args]);
    const fail = this.failNext;
    this.failNext = false;
    this.arrived?.();
    return new Promise<void>((resolve, reject) => {
      this.waiting.push({ resolve, reject: (error) => reject(error) });
      if (fail) this.waiting.pop()?.reject(new Error("pane_not_found: pane w1:p4 not found"));
    });
  };

  async until(count: number): Promise<void> {
    while (this.calls.length < count) await new Promise<void>((resolve) => { this.arrived = resolve; });
  }

  finish(): void {
    const call = this.waiting.shift();
    if (!call) throw new Error("no herdr call is in flight");
    call.resolve();
  }

  async drain(count: number): Promise<void> {
    while (this.calls.length < count || this.waiting.length > 0) {
      if (this.waiting.length > 0) this.finish();
      else await this.until(this.calls.length + 1);
    }
  }

  states(): string[] {
    return this.calls.filter((c) => c[1] === "report-agent").map((c) => c[c.indexOf("--state") + 1] ?? "");
  }

  seqs(): bigint[] {
    return this.calls.map((c) => BigInt(c[c.indexOf("--seq") + 1] ?? "-1"));
  }
}

function lifecycle(identity: HostIdentity = WORKER) {
  const herdr = new GatedHerdr();
  const events = new EventLog(identity.generation);
  const adapter = new HerdrLifecycle({ bin: "/opt/herdr", pane: "w1:p4" }, identity,
    (kind, data) => events.emit(kind, data), herdr.run);
  return { herdr, events, adapter };
}

const bind = (adapter: HerdrLifecycle, questions: unknown = []) =>
  adapter.bind({ sessionId: "019a-sess", sessionFile: "/s/019a-sess.jsonl", epochMs: EPOCH, questions });

test("bind names the identity before the first state, then turns report working and idle", async () => {
  const { herdr, adapter } = lifecycle();
  adapter.turnStarted();
  expect(herdr.calls).toEqual([]);

  bind(adapter);
  await herdr.drain(2);
  expect(herdr.calls[0]).toEqual(["pane", "report-metadata", "w1:p4", "--source", SOURCE, "--agent", AGENT,
    "--applies-to-source", SOURCE, "--display-agent", "corral-pi worker T-7/R2 g3",
    "--token", "corral_role=worker", "--token", "corral_project=app", "--token", "corral_task=T-7",
    "--token", "corral_run=R2", "--clear-token", "corral_review", "--clear-token", "corral_assistant",
    "--token", "corral_generation=3", "--token", "corral_session=019a-sess", "--seq", `${BASE + 1n}`]);
  expect(herdr.calls[1]).toEqual(["pane", "report-agent", "w1:p4", "--source", SOURCE, "--agent", AGENT,
    "--state", "working", "--agent-session-id", "019a-sess", "--agent-session-path", "/s/019a-sess.jsonl",
    "--seq", `${BASE + 2n}`]);

  adapter.turnEnded();
  await herdr.drain(3);
  expect(herdr.states()).toEqual(["working", "idle"]);
});

test("changes during a call coalesce to the latest state, and equivalent events report nothing", async () => {
  const { herdr, adapter } = lifecycle();
  bind(adapter);
  await herdr.drain(2);
  expect(herdr.states()).toEqual(["idle"]);

  adapter.turnEnded();
  adapter.turnEnded();
  expect(herdr.calls).toHaveLength(2);

  adapter.turnStarted();
  await herdr.until(3);
  adapter.turnEnded();
  adapter.turnStarted();
  adapter.turnEnded();
  adapter.turnStarted();
  herdr.finish();
  await herdr.drain(3);
  expect(herdr.states()).toEqual(["idle", "working"]);

  adapter.turnStarted();
  adapter.turnEnded();
  await herdr.drain(4);
  expect(herdr.states()).toEqual(["idle", "working", "idle"]);
  const seqs = herdr.seqs();
  expect(seqs.every((seq, i) => i === 0 || seq > (seqs[i - 1] ?? 0n))).toBeTrue();
});

test("an owner question blocks through turns, provider errors and cancellation until answered or withdrawn", async () => {
  const { herdr, adapter } = lifecycle();
  bind(adapter);
  await herdr.drain(2);

  adapter.turnStarted();
  adapter.asked({ key: "T-7/R2/Q1", kind: "clarification", blocking: false });
  await herdr.drain(3);
  expect(herdr.states()).toEqual(["idle", "working"]);

  adapter.asked({ key: "T-7/R2/Q2", kind: "owner_only", blocking: false });
  await herdr.drain(4);
  const blocked = herdr.calls[3] ?? [];
  expect(blocked[blocked.indexOf("--message") + 1]).toBe("corral question T-7/R2/Q2 (owner_only) awaits an answer");

  // A cancelled or failed turn, and nested turns, settle the turn but not the question.
  adapter.turnEnded();
  adapter.turnStarted();
  adapter.turnStarted();
  adapter.turnEnded();
  adapter.resolved("T-7/R2/Q1");
  adapter.resolved("T-7/R2/Q9");
  expect(herdr.calls).toHaveLength(4);

  adapter.asked({ key: "T-7/R2/Q3", kind: "clarification", blocking: true });
  await herdr.drain(5);
  const both = herdr.calls[4] ?? [];
  expect(both[both.indexOf("--message") + 1]).toBe("corral question T-7/R2/Q2 (owner_only) and 1 more awaits an answer");

  adapter.resolved("T-7/R2/Q2");
  await herdr.drain(6);
  adapter.resolved("T-7/R2/Q3");
  await herdr.drain(7);
  expect(herdr.states()).toEqual(["idle", "working", "blocked", "blocked", "blocked", "idle"]);
});

test("a resumed host starts blocked on the hello receipt's pending owner or escalated questions", async () => {
  const { herdr, adapter } = lifecycle();
  bind(adapter, [
    { key: "T-7/R2/Q1", kind: "clarification", blocking: false, state: "open" },
    { key: "T-7/R2/Q2", kind: "clarification", blocking: false, state: "escalated" },
    { nonsense: true },
  ]);
  await herdr.drain(2);
  expect(herdr.states()).toEqual(["blocked"]);
  expect(adapter.readback().blocked_on).toEqual(["T-7/R2/Q2"]);
});

test("graceful close waits for the call in flight, then releases and clears; later events report nothing", async () => {
  const { herdr, adapter } = lifecycle();
  bind(adapter);
  await herdr.drain(2);
  adapter.turnStarted();
  await herdr.until(3);

  const closed = adapter.close();
  let second = false;
  void adapter.close().then(() => { second = true; });
  adapter.turnEnded();
  herdr.finish();
  await herdr.until(4);
  expect(herdr.calls[3]).toEqual(["pane", "release-agent", "w1:p4", "--source", SOURCE, "--agent", AGENT,
    "--seq", `${BASE + 4n}`]);
  herdr.finish();
  await herdr.until(5);
  expect(herdr.calls[4]).toEqual(["pane", "report-metadata", "w1:p4", "--source", SOURCE, "--clear-display-agent",
    ...TOKENS.flatMap((token) => ["--clear-token", token]), "--seq", `${BASE + 5n}`]);
  expect(second).toBeFalse();
  herdr.finish();
  await closed;
  expect(second).toBeTrue();

  adapter.turnStarted();
  adapter.asked({ key: "T-7/R2/Q1", kind: "owner_only", blocking: true });
  await adapter.close();
  expect(herdr.calls).toHaveLength(5);
  expect(herdr.states()).toEqual(["idle", "working"]);
});

test("a failed report is visible and the next event reports the latest state", async () => {
  const { herdr, events, adapter } = lifecycle();
  bind(adapter);
  await herdr.drain(2);
  const failure = nextEvent(events, (event) => event.kind === "herdr_report_failed");
  herdr.failNext = true;
  adapter.turnStarted();
  const failed = await failure;
  expect([failed.op, failed.reason]).toEqual(["state", "pane_not_found: pane w1:p4 not found"]);
  const readback = adapter.readback();
  expect([readback.failed, readback.reported, readback.state]).toEqual([1, "idle", "working"]);

  adapter.asked({ key: "T-7/R2/Q1", kind: "permission", blocking: true });
  await herdr.drain(4);
  expect(herdr.states()).toEqual(["idle", "working", "blocked"]);
});

test("without a hello epoch nothing is reported, and the reason is visible", async () => {
  const { herdr, events, adapter } = lifecycle();
  adapter.bind({ sessionId: "s", sessionFile: "/s.jsonl", epochMs: undefined, questions: [] });
  adapter.turnStarted();
  await adapter.close();
  expect(herdr.calls).toEqual([]);
  expect(events.since(0, 64 * 1024).events.map((event) => event.kind)).toEqual(["herdr_report_failed"]);
});

test("every role names its own corral identity through the one adapter", async () => {
  const roles: [HostIdentity, string, string[]][] = [
    [{ ...WORKER, role: "reviewer", review: "review1" }, "corral-pi reviewer T-7/R2 review1 g3", ["corral_review=review1"]],
    [{ ...WORKER, role: "planner", run: "P1" }, "corral-pi planner T-7/P1 g3", ["corral_run=P1"]],
    [{ ...WORKER, role: "plan_reviewer", run: "P1", review: "PR1" }, "corral-pi plan_reviewer T-7/P1 PR1 g3", ["corral_review=PR1"]],
    [{ store: "st-1", project: "app", role: "pm", generation: 2 }, "corral-pi pm app g2", ["corral_project=app"]],
    [{ store: "st-1", project: "", role: "assistant", assistant: "A-4", generation: 1 }, "corral-pi assistant A-4 g1",
      ["corral_assistant=A-4"]],
  ];
  for (const [identity, display, tokens] of roles) {
    const { herdr, adapter } = lifecycle(identity);
    bind(adapter);
    await herdr.drain(2);
    const metadata = herdr.calls[0] ?? [];
    expect(metadata[metadata.indexOf("--display-agent") + 1]).toBe(display);
    for (const token of [`corral_role=${identity.role}`, ...tokens]) expect(metadata).toContain(token);
    expect(herdr.calls[1]?.slice(0, 7)).toEqual(["pane", "report-agent", "w1:p4", "--source", SOURCE, "--agent", AGENT]);
  }
});

test("only a host inside its bound herdr pane reports, and a headless host never does", () => {
  const env = { HERDR_ENV: "1", HERDR_BIN_PATH: "/opt/herdr", HERDR_PANE_ID: "w1:p4", HERDR_SOCKET_PATH: "/run/h.sock" };
  expect(herdrTarget(env, "w1:p4")).toEqual({ bin: "/opt/herdr", pane: "w1:p4" });
  expect(herdrTarget({ ...env, HERDR_ENV: undefined }, "w1:p4")).toBe("not_in_herdr");
  expect(herdrTarget({ ...env, HERDR_SOCKET_PATH: "" }, "w1:p4")).toBe("herdr_environment_incomplete");
  expect(herdrTarget(env, "w1:p5")).toBe("pane_mismatch");

  const events = new EventLog(1);
  expect(herdrLifecycle(env, "w1:p4", WORKER, events, true)).toEqual({});
  expect(herdrLifecycle(env, "w1:p4", WORKER, events, false).herdr instanceof HerdrLifecycle).toBeTrue();
  expect(events.since(0, 64 * 1024).events.map((event) => [event.kind, event.reason])).toEqual([["herdr_reporting_off", "headless"]]);
});

test("corral's cancellation and escalation updates move only their own question, newest revision first", async () => {
  const { herdr, adapter } = lifecycle();
  bind(adapter);
  await herdr.drain(2);
  adapter.asked({ key: "T-7/R2/Q1", kind: "owner_only", blocking: true, revision: 1 });
  adapter.asked({ key: "T-7/R2/Q2", kind: "clarification", blocking: false, revision: 1 });
  adapter.asked({ key: "T-7/R2/Q3", kind: "permission", blocking: true, revision: 1 });
  await herdr.drain(3);
  expect(adapter.readback().blocked_on).toEqual(["T-7/R2/Q1", "T-7/R2/Q3"]);

  // An owner or PM cancellation of Q1 releases Q1 alone; Q3 still blocks.
  adapter.apply([
    { key: "T-7/R2/Q1", kind: "owner_only", blocking: true, state: "cancelled", revision: 2 },
    { key: "T-7/R2/Q3", kind: "permission", blocking: true, state: "open", revision: 1 },
  ]);
  await herdr.drain(4);
  expect(adapter.readback().blocked_on).toEqual(["T-7/R2/Q3"]);

  // Escalating the non-blocking Q2 makes it block.
  adapter.apply([{ key: "T-7/R2/Q2", kind: "clarification", blocking: false, state: "escalated", revision: 2 }]);
  await herdr.drain(5);
  expect(adapter.readback().blocked_on).toEqual(["T-7/R2/Q3", "T-7/R2/Q2"]);

  // Late or repeated updates, and a replayed ask, never undo a newer fact.
  adapter.resolved("T-7/R2/Q3", 2);
  adapter.resolved("T-7/R2/Q2", 3);
  await herdr.drain(6);
  adapter.apply([
    { key: "T-7/R2/Q1", kind: "owner_only", blocking: true, state: "open", revision: 1 },
    { key: "T-7/R2/Q2", kind: "clarification", blocking: false, state: "escalated", revision: 2 },
    { key: "T-7/R2/Q3", kind: "permission", blocking: true, state: "open", revision: 2 },
  ]);
  adapter.asked({ key: "T-7/R2/Q1", kind: "owner_only", blocking: true, revision: 1 });
  expect(herdr.calls).toHaveLength(6);
  expect(herdr.states()).toEqual(["idle", "blocked", "blocked", "blocked", "idle"]);
  expect(adapter.readback().blocked_on).toEqual([]);
});

/**
 * herdr 0.9.5's agent and metadata trackers as observed: per source, a report whose sequence
 * number is not above the last accepted one is acknowledged and ignored.
 */
function modeledHerdr() {
  const last = new Map<string, bigint>();
  const pane = { agent: null as string | null, state: null as string | null, display: null as string | null };
  let calls = 0;
  let waiters: { count: number; resolve: () => void }[] = [];
  const run = async (_bin: string, args: readonly string[]) => {
    const seq = BigInt(args[args.indexOf("--seq") + 1] ?? "0");
    const tracker = args[1] === "report-metadata" ? "metadata" : "agent";
    if (seq > (last.get(tracker) ?? -1n)) {
      last.set(tracker, seq);
      if (args[1] === "report-agent") Object.assign(pane, { agent: AGENT, state: args[args.indexOf("--state") + 1] });
      if (args[1] === "release-agent") Object.assign(pane, { agent: null, state: null });
      if (args[1] === "report-metadata") {
        pane.display = args.includes("--clear-display-agent") ? null : args[args.indexOf("--display-agent") + 1] ?? null;
      }
    }
    calls++;
    const ready = waiters.filter((waiter) => calls >= waiter.count);
    waiters = waiters.filter((waiter) => calls < waiter.count);
    for (const waiter of ready) waiter.resolve();
  };
  const until = (count: number) => calls >= count ? Promise.resolve()
    : new Promise<void>((resolve) => { waiters.push({ count, resolve }); });
  const shown = () => [pane.agent, pane.state, pane.display];
  return { run, until, shown };
}

function generations(firstEpoch: number, secondEpoch: number) {
  const model = modeledHerdr();
  const target = { bin: "/opt/herdr", pane: "w1:p4" };
  const g1 = new HerdrLifecycle(target, WORKER, () => undefined, model.run);
  const g2 = new HerdrLifecycle(target, { ...WORKER, generation: 4 }, () => undefined, model.run);
  const bindAt = (adapter: HerdrLifecycle, epochMs: number) =>
    adapter.bind({ sessionId: "019a-sess", sessionFile: "/s/019a-sess.jsonl", epochMs, questions: [] });
  return { model, g1, g2, bindAt };
}

test("a replacement's allocated epoch fences the old generation's late reports, release and cleanup", async () => {
  // Given generation 1 working, then generation 2 bound at the epoch corral allocated after it
  // (one higher even when the wall clock stepped back) and working too:
  const { model, g1, g2, bindAt } = generations(EPOCH, EPOCH + 1);
  bindAt(g1, EPOCH);
  g1.turnStarted();
  await model.until(2);
  bindAt(g2, EPOCH + 1);
  g2.turnStarted();
  await model.until(4);
  expect(model.shown()).toEqual([AGENT, "working", "corral-pi worker T-7/R2 g4"]);

  // When generation 1 settles late, then exits and releases and clears the pane:
  g1.turnEnded();
  await model.until(5);
  await g1.close();

  // Then herdr still shows generation 2, working, under its own name:
  expect(model.shown()).toEqual([AGENT, "working", "corral-pi worker T-7/R2 g4"]);
});

test("an epoch below the predecessor's, as a raw backward wall clock would give, is ignored by herdr", async () => {
  // Given generation 1 working, and generation 2 bound 100 ms "earlier" than it:
  const { model, g1, g2, bindAt } = generations(EPOCH, EPOCH - 100);
  bindAt(g1, EPOCH);
  g1.turnStarted();
  await model.until(2);
  bindAt(g2, EPOCH - 100);
  g2.turnStarted();
  await model.until(4);

  // Then none of generation 2's reports land: the reason corral allocates epochs monotonically.
  expect(model.shown()).toEqual([AGENT, "working", "corral-pi worker T-7/R2 g3"]);
});

test("an inbox envelope names every question its answer or correction members settle", () => {
  const envelope = [
    "[corral envelope T-7_R2@env4]",
    "[corral T-7_R2#9 question_answer]\nAnswer to T-7/R2/Q1 (revision 2) by Pm:\nUse the inbox.",
    "[corral T-7_R2#10 reply]\nAnswer to T-7/R2/Q5 (revision 1) by Owner:\nquoted in a reply, not an answer",
    "[corral T-7_R2#11 question_correction]\nCorrected answer to T-7/R2/Q2 (revision 3) by Owner:\nNo.\nChosen option: deny",
  ].join("\n\n");
  expect(answeredKeys(envelope)).toEqual([{ key: "T-7/R2/Q1", revision: 2 }, { key: "T-7/R2/Q2", revision: 3 }]);
  expect(answeredKeys("Answer to T-7/R2/Q1 (revision 2) by Owner:\nunframed")).toEqual([]);
  expect(answeredKeys("[corral T-7_R2#3 turn_nudge]\nPlease continue.")).toEqual([]);
});
