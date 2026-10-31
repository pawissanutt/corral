// Real PM and assistant hosts over the fake corral and the fake model: the canonical plan reaches
// every model request, and corral's turn_ended decision starts at most one continuation turn.
import { afterEach, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { type FakeCorral, writeFakeCorral } from "./support/fake-corral.ts";
import { PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { nextEvent, within } from "./support/wait.ts";

type SessionRole = "pm" | "assistant";
const ROLES: readonly SessionRole[] = ["pm", "assistant"];

function progress(revision: number, plan = "P1", extra: Record<string, unknown> = {}) {
  return { revision, spec_revision: "spec", plan, phases: [{ id: "work", label: "Work", steps: [
    { id: "a", label: "Read the board", status: { state: "in_progress" }, criteria: [] },
    { id: "b", label: "Reply to the owner", status: { state: "pending" }, criteria: [] },
  ] }], ...extra };
}

function snapshot(role: SessionRole, todo: unknown, requests: unknown = null) {
  return role === "pm" ? { container: { todo, todo_requests: requests } }
    : { container: { state: { todo, todo_requests: requests } } };
}

const request = (revision: number, continuation: Record<string, unknown>, end = "normal") => ({ request: {
  plan: "P1", revision, end, disposition: { state: "actionable", next: "a" }, continuation,
  next: { id: "a", label: "Read the board" },
} });
const due = (revision: number, attempt = 1) =>
  request(revision, { state: "due", key: `continue:P1:${revision}`, attempt, max: 8 });

/** A client whose next ledger read waits until the test releases it. */
class GatedClient extends CorralClient {
  private gate: { entered: () => void; released: Promise<void> } | undefined;

  holdNextRead(): { entered: Promise<void>; release: () => void } {
    let entered!: () => void;
    let release!: () => void;
    const reached = new Promise<void>((resolve) => { entered = resolve; });
    this.gate = { entered, released: new Promise<void>((resolve) => { release = resolve; }) };
    return { entered: reached, release };
  }

  override async todoSnapshot(): Promise<unknown> {
    const gate = this.gate;
    this.gate = undefined;
    if (gate) {
      gate.entered();
      await gate.released;
    }
    return super.todoSnapshot();
  }
}

interface Started {
  readonly host: HostSession;
  readonly corral: FakeCorral;
  readonly client: GatedClient;
  readonly events: EventLog;
}

const cleanups: (() => void)[] = [];
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); });

async function start(role: SessionRole, options: {
  todo?: boolean; state?: unknown; turnEnded?: Record<string, unknown>[]; compaction?: boolean;
} = {}): Promise<Started> {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-request-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  cleanups.push(() => {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  });
  for (const tree of [`src/roles/${role}`, "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  const cwd = join(work, "cwd");
  mkdirSync(cwd);
  restrictEnvironment(process.env);
  const identity = role === "pm"
    ? { store: "st-demo", project: "demo", role, generation: 1 }
    : { store: "st-demo", project: "", role, assistant: "A-1", generation: 1 };
  const modules = await loadModules(assets.dir, role);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("fixture transport missing");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const events = new EventLog(1);
  const corral = writeFakeCorral(work);
  corral.snapshot(options.state ?? snapshot(role, progress(2)));
  if (options.turnEnded) corral.turnEnded(options.turnEnded);
  const client = new GatedClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64),
    sessionId: () => host?.sessionId() ?? "" });
  // A generation snapshot with compaction on and a tiny recent window lets a short session compact.
  const launch = options.compaction
    ? { capabilities: { catalog: 1, digest: "d", preset: "standard", enabled: ["todo", "compaction"] },
      guidance: { settings: { compaction: { keep_recent_tokens: 1 } }, projectText: false } }
    : { todo: options.todo ?? true };
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", ...launch,
    modules, client, events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
  return { host, corral, client, events };
}

/** Submits one bridge prompt and returns once its turn ended and corral answered it. */
async function turn(s: Started, text: string, key: string, number: number): Promise<void> {
  const ended = nextEvent(s.events, (event) => event.kind === "turn_ended" && event.turn === number);
  expect(await s.host.submit({ text, busyPolicy: "prompt", idempotencyKey: key, origin: "test" }, `rq-${key}`))
    .toEqual({ disposition: "started", turn: number });
  await ended;
  await settled(s);
}

/** Every report and the continuation decision it queued have finished. */
async function settled(s: Started): Promise<void> {
  await s.host.drained();
  await s.host.drained();
}

const lifecycle = (s: Started) => s.corral.calls().map((call) => call.envelope)
  .filter((envelope) => envelope.op === "event").map((envelope) => envelope.payload);
const started = (s: Started) => lifecycle(s).filter((payload) => payload.kind === "turn_started");
const lastPrompt = () => fixtureTransport().prompts.at(-1) ?? "";

for (const role of ROLES) {
  test(`${role}: a started or resumed host restores the plan and every model request carries its summary`, async () => {
    // Given a canonical plan P2 at revision 5 whose second step waits on an event.
    const waits = [{ step: "b", on: { kind: "event", event: "T-12 lands" } }];
    const s = await start(role, { state: snapshot(role, progress(5, "P2", { waits })) });
    // When the first turn reaches the model.
    await turn(s, "Reply only READY.", "k1", 1);
    // Then the prompt carries the plan identity, revision, next action and the wait.
    const prompt = lastPrompt();
    expect(prompt).toContain("[corral request P2 revision 5");
    expect(prompt).toContain("next step a");
    expect(prompt).toContain("T-12 lands");
    expect(prompt).toContain("Reply only READY.");
    // And the summary is context only: Pi's session file never records it.
    expect(readFileSync(s.host.session.sessionFile!, "utf8")).not.toContain("[corral request P2 revision 5");
  }, 60_000);

  test(`${role}: newer receipts replace the summary while stale receipts and compaction cannot lose it`, async () => {
    // Given plan P1 at revision 2, and a turn whose model finishes step a (the fake moves it to 3).
    const s = await start(role, { compaction: true });
    fixtureTransport().script([
      { toolCalls: [{ callId: "done-a", name: "corral_todo", arguments: { op: "done", step: "a", plan: "P1" } }] },
      { text: "Step a is done." },
    ]);
    await turn(s, "Finish step a.", "k1", 1);
    // When a stale snapshot is replayed through a later receipt.
    s.corral.snapshot(snapshot(role, progress(1)));
    fixtureTransport().script([
      { toolCalls: [{ callId: "late", name: "corral_todo", arguments: { op: "start", step: "b" } }] },
      { text: "Started." },
    ]);
    await turn(s, "Reply only SECOND.", "k2", 2);
    expect(lastPrompt()).toContain("[corral request P1 revision 3");
    // And the transcript is compacted before the next turn.
    fixtureTransport().script([{ text: "SUMMARY of the session" }]);
    await s.host.session.compact();
    const entries = readFileSync(s.host.session.sessionFile!, "utf8").trim().split("\n")
      .map((line) => JSON.parse(line) as { type?: string });
    expect(entries.some((entry) => entry.type === "compaction")).toBeTrue();
    await turn(s, "Reply only THIRD.", "k3", 3);
    // Then the model still gets the newest canonical revision, not the stale receipt's.
    expect(lastPrompt()).toContain("[corral request P1 revision 3");
    expect(lastPrompt()).not.toContain("revision 1");
  }, 60_000);

  test(`${role}: a due continuation starts one continuation turn and a duplicate decision none`, async () => {
    // Given corral grants a continuation for P1 revision 2, stops it after that turn, then repeats it.
    const stop = request(2, { state: "stopped", reason: "no_progress" });
    const s = await start(role, { turnEnded: [due(2), stop, due(2), stop] });
    const continued = nextEvent(s.events, (event) => event.kind === "turn_ended" && event.turn === 2);
    const stopped = nextEvent(s.events, (event) => event.kind === "continuation_stopped");
    // When the owner's turn ends.
    await turn(s, "Reply only FIRST.", "k1", 1);
    await continued;
    await settled(s);
    // Then exactly one continuation turn ran under its corral key, given the next action.
    expect(started(s)[1]?.origin).toEqual({ kind: "continuation", idempotency_key: "continue:P1:2" });
    expect(fixtureTransport().prompts.some((prompt) => prompt.includes("[corral continuation P1 revision 2"))).toBeTrue();
    expect((await stopped).reason).toBe("no_progress");
    // When a later turn's receipt grants the same continuation key again.
    await settled(s);
    await turn(s, "Reply only THIRD.", "k3", 3);
    // Then no further turn starts.
    expect(started(s)).toHaveLength(3);
    // And a later turn at the stopped revision does not repeat the owner's notice.
    await turn(s, "Reply only FOURTH.", "k4", 4);
    const kinds = s.events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events.map((event) => event.kind);
    expect(kinds.filter((kind) => kind === "continuation")).toHaveLength(1);
    expect(kinds.filter((kind) => kind === "continuation_stopped")).toHaveLength(1);
  }, 60_000);

  test(`${role}: an owner instruction, a newer revision and held turns all win over a due continuation`, async () => {
    // Given a due continuation whose recheck is held until the owner has started a new turn.
    const s = await start(role, { turnEnded: [due(2), {}, due(3), request(4, { state: "held" }, "provider_failed")] });
    const transport = fixtureTransport();
    const gate = s.client.holdNextRead();
    const busy = nextEvent(s.events, (event) => event.kind === "continuation_skipped");
    const ended = nextEvent(s.events, (event) => event.kind === "turn_ended" && event.turn === 1);
    await s.host.submit({ text: "Reply only FIRST.", busyPolicy: "prompt", idempotencyKey: "k1", origin: "test" }, "rq-k1");
    await ended;
    await within(gate.entered, 30_000, "the continuation's recheck");
    // When the owner starts a turn, held open by the model, before the recheck returns.
    transport.holdNext();
    const held = transport.whenHeld();
    const owner = nextEvent(s.events, (event) => event.kind === "turn_ended" && event.turn === 2);
    expect(await s.host.submit({ text: "Reply only OWNER.", busyPolicy: "prompt", idempotencyKey: "k2", origin: "test" },
      "rq-k2")).toEqual({ disposition: "started", turn: 2 });
    await within(held, 30_000, "the owner's open turn");
    gate.release();
    // Then the owner's turn wins and the continuation is dropped as busy.
    expect((await busy).reason).toBe("busy");
    transport.release();
    await owner;
    await settled(s);
    // When the canonical plan moved past the granted revision.
    s.corral.snapshot(snapshot(role, progress(4)));
    const superseded = nextEvent(s.events, (event) => event.kind === "continuation_skipped" && event.reason === "superseded");
    await turn(s, "Reply only THIRD.", "k3", 3);
    await superseded;
    // When a failed turn's decision is held.
    fixtureTransport().failNext();
    await turn(s, "Reply only FAILED.", "k4", 4);
    // Then no continuation turn ever started.
    expect(started(s).map((payload) => (payload.origin as { kind: string }).kind)).not.toContain("continuation");
  }, 60_000);

  test(`${role}: a cancelled turn reports its abort and is never continued`, async () => {
    // Given a turn held open by the model and corral's held decision for it.
    const s = await start(role, { turnEnded: [request(2, { state: "held" }, "cancelled")] });
    const transport = fixtureTransport();
    transport.holdNext();
    const held = transport.whenHeld();
    const ended = nextEvent(s.events, (event) => event.kind === "turn_ended" && event.turn === 1);
    await s.host.submit({ text: "Reply only HELD.", busyPolicy: "prompt", idempotencyKey: "k1", origin: "test" }, "rq-k1");
    await within(held, 30_000, "the held response");
    // When corral cancels it.
    expect(await s.host.cancel(1)).toEqual({ cancelled: true, turn: 1 });
    await ended;
    await settled(s);
    // Then corral is told it was cancelled and aborted, and no other turn starts.
    expect(lifecycle(s).find((payload) => payload.kind === "turn_ended"))
      .toEqual({ kind: "turn_ended", turn: 1, cancelled: true, aborted: true });
    expect(started(s)).toHaveLength(1);
  }, 60_000);

  test(`${role}: with todo off there is no plan summary and no continuation`, async () => {
    // Given todo disabled for the generation, even if a receipt claimed a continuation.
    const s = await start(role, { todo: false, turnEnded: [due(2)] });
    expect(s.host.loadout).not.toContain("corral_todo");
    // When a turn ends.
    await turn(s, "Reply only OFF.", "k1", 1);
    // Then the model got no request summary and no continuation turn started.
    expect(lastPrompt()).not.toContain("[corral request");
    expect(started(s)).toHaveLength(1);
  }, 60_000);
}
