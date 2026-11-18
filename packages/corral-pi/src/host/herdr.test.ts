// The shared herdr adapter (T-605), driven through a gated fake herdr: each test holds a call in
// flight to observe exactly what is reported, in which order and with which sequence numbers.
import { expect, test } from "bun:test";

import { EventLog } from "../bridge/events.ts";
import type { HostIdentity } from "../roles/types.ts";
import {
  AGENT, HerdrLifecycle, SEQ_SHIFT, SOURCE, TOKENS, herdrLifecycle, herdrTarget,
} from "./herdr.ts";
import { nextEvent } from "../../test/support/wait.ts";

const EPOCH = 1_793_000_000_123;
const BASE = BigInt(EPOCH) << SEQ_SHIFT;
const WORKER: HostIdentity = { store: "st-1", project: "app", role: "worker", task: "T-7", run: "R2" };

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
  const events = new EventLog();
  const adapter = new HerdrLifecycle({ bin: "/opt/herdr", pane: "w1:p4" }, identity,
    (kind, data) => events.emit(kind, data), herdr.run);
  return { herdr, events, adapter };
}

function bind(adapter: HerdrLifecycle, now = EPOCH): void {
  const clock = Date.now;
  Date.now = () => now;
  try {
    adapter.bind({ sessionId: "019a-sess", sessionFile: "/s/019a-sess.jsonl" });
  } finally {
    Date.now = clock;
  }
}

test("bind names the identity before the first state, then turns report working and idle", async () => {
  const { herdr, adapter } = lifecycle();
  adapter.turnStarted();
  expect(herdr.calls).toEqual([]);

  bind(adapter);
  await herdr.drain(2);
  expect(herdr.calls[0]).toEqual(["pane", "report-metadata", "w1:p4", "--source", SOURCE, "--agent", AGENT,
    "--applies-to-source", SOURCE, "--display-agent", "corral-pi worker T-7/R2",
    "--token", "corral_role=worker", "--token", "corral_project=app", "--token", "corral_task=T-7",
    "--token", "corral_run=R2", "--clear-token", "corral_assistant",
    "--token", "corral_session=019a-sess", "--seq", `${BASE + 1n}`]);
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

test("local owner questions keep a pane blocked until the last pending question resolves", async () => {
  const { herdr, adapter } = lifecycle();
  bind(adapter);
  await herdr.drain(2);

  adapter.turnStarted();
  await herdr.drain(3);
  expect(herdr.states()).toEqual(["idle", "working"]);

  adapter.questionChanged("q-first", true);
  await herdr.drain(4);
  const blocked = herdr.calls[3] ?? [];
  expect(blocked[blocked.indexOf("--message") + 1]).toContain("q-first");

  // Settling and starting more turns does not resolve a pending question.
  adapter.turnEnded();
  adapter.turnStarted();
  adapter.turnStarted();
  adapter.turnEnded();
  adapter.questionChanged("q-first", true);
  adapter.questionChanged("q-unknown", false);
  expect(herdr.calls).toHaveLength(4);

  adapter.questionChanged("q-second", true);
  await herdr.drain(5);
  expect(adapter.readback().blocked_on).toEqual(["q-first", "q-second"]);

  adapter.questionChanged("q-first", false);
  await herdr.drain(6);
  expect(adapter.readback().blocked_on).toEqual(["q-second"]);
  adapter.questionChanged("q-second", false);
  await herdr.drain(7);
  expect(herdr.states()).toEqual(["idle", "working", "blocked", "blocked", "blocked", "idle"]);
});

test("the first state includes local questions restored before bind", async () => {
  const { herdr, adapter } = lifecycle();
  adapter.questionChanged("q-restored", true);
  expect(herdr.calls).toEqual([]);
  bind(adapter);
  await herdr.drain(2);
  expect(herdr.states()).toEqual(["blocked"]);
  expect(adapter.readback().blocked_on).toEqual(["q-restored"]);
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
  adapter.questionChanged("q-after-close", true);
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

  adapter.questionChanged("q-permission", true);
  await herdr.drain(4);
  expect(herdr.states()).toEqual(["idle", "working", "blocked"]);
});

test("every role names its own corral identity through the one adapter", async () => {
  const roles: [HostIdentity, string, string[]][] = [
    [{ ...WORKER, role: "reviewer" }, "corral-pi reviewer T-7/R2", ["corral_run=R2"]],
    [{ store: "st-1", project: "", role: "assistant", assistant: "A-4" }, "corral-pi assistant A-4",
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

  const events = new EventLog();
  expect(herdrLifecycle(env, "w1:p4", WORKER, events, true)).toEqual({});
  expect(herdrLifecycle(env, "w1:p4", WORKER, events, false).herdr instanceof HerdrLifecycle).toBeTrue();
  expect(events.since(0, 64 * 1024).events.map((event) => [event.kind, event.reason])).toEqual([["herdr_reporting_off", "headless"]]);
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

test("a host bound at a later time is not overwritten by an earlier host's reports or release", async () => {
  const model = modeledHerdr();
  const target = { bin: "/opt/herdr", pane: "w1:p4" };
  const first = new HerdrLifecycle(target, WORKER, () => undefined, model.run);
  const replacement = new HerdrLifecycle(target, WORKER, () => undefined, model.run);
  bind(first);
  await model.until(2);
  bind(replacement, EPOCH + 1);
  await model.until(4);
  expect(model.shown()).toEqual([AGENT, "idle", "corral-pi worker T-7/R2"]);

  first.turnStarted();
  await model.until(5);
  await first.close();
  expect(model.shown()).toEqual([AGENT, "idle", "corral-pi worker T-7/R2"]);
});
