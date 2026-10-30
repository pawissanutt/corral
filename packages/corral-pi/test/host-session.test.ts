import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { startBridge } from "../src/bridge/server.ts";
import { loadPeerLookup } from "../src/host/peer.ts";
import { connect } from "./support/bridge-client.ts";
import type { SubmitInput } from "../src/bridge/protocol.ts";
import { loadModules } from "../src/host/modules.ts";
import { BUILTIN_EXTENSIONS, NetworkRefused, guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { forbiddenRuns, holdNextInput } from "./fixtures/roles/fixture-role.ts";
import { type FakeCorral, writeFakeCorral } from "./support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "./support/fixture-install.ts";
import { expectReplacementsRefused } from "./support/replacement.ts";
import { nextEvent, within } from "./support/wait.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
const CTL = "c".repeat(64);
const NONCE = "n".repeat(64);
const originalFetch = globalThis.fetch;

let assets: FixtureInstall;
let work: string;
let corral: FakeCorral;
let events: EventLog;
let host: HostSession;

const input = (text: string, busyPolicy: SubmitInput["busyPolicy"], key: string): SubmitInput =>
  ({ text, busyPolicy, idempotencyKey: key, origin: "test" });

beforeAll(async () => {
  assets = buildFixtureInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-host-"));
  const cwd = join(work, "worktree");
  mkdirSync(join(cwd, ".pi/skills/leak"), { recursive: true });
  mkdirSync(join(cwd, ".pi/prompts"), { recursive: true });
  writeFileSync(join(cwd, "AGENTS.md"), "Inherited context that must not load.\n");
  writeFileSync(join(cwd, ".pi/skills/leak/SKILL.md"), "---\nname: leak\ndescription: must not load\n---\nLeak.\n");
  writeFileSync(join(cwd, ".pi/prompts/leak.md"), "Leaked prompt template.\n");
  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "worker");
  events = new EventLog(identity.generation);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport, (destination) => events.emit("network_refused", { destination }));
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  corral = writeFakeCorral(work);
  const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host.sessionId() });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
}, 60_000);

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("input before hello is rejected; hello binds the generation through corral", async () => {
  expect(await host.submit(input("Reply only EARLY.", "prompt", "k0"), "rq-0")).toEqual({ disposition: "rejected", reason: "host_not_bound" });
  await host.hello(NONCE, "pane-1", sha256Hex(CTL));
  expect(host.bound).toBeTrue();
  const [hello] = corral.calls();
  expect(hello?.argv).toEqual(["agent", "call", "--op", "hello"]);
  expect(hello?.envelope.request_id).toBe("ev-1-0");
  expect(hello?.envelope.auth.bind_nonce).toBe(NONCE);
  expect(hello?.envelope.payload.ctl_cap_sha256).toBe(sha256Hex(CTL));
  expect(String(hello?.envelope.payload.session_file).startsWith(join(work, "sessions"))).toBeTrue();
  expect(hello?.envelope.payload.pane).toBe("pane-1");
  expect(hello?.stdin).not.toContain(CTL);
});

test("idle input starts a turn, reported to corral as turn_started and turn_ended", async () => {
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
  expect(await host.submit(input("Reply only STARTED.", "prompt", "k1"), "rq-1")).toEqual({ disposition: "started", turn: 1 });
  await ended;
  await host.drained();
  const lifecycle = corral.calls().filter((call) => call.envelope.op === "event").map((call) => call.envelope.payload);
  expect(lifecycle).toEqual([
    { kind: "turn_started", turn: 1, origin: { kind: "bridge", idempotency_key: "k1" }, provider: "openai-codex", model: "gpt-6.1-sol" },
    { kind: "turn_ended", turn: 1 },
  ]);
});

test("busy input is queued for steer and follow_up, rejected for prompt, and never claims completion", async () => {
  const transport = fixtureTransport();
  expect(await host.submit(input("/fixture-hold", "prompt", "k2"), "rq-2")).toEqual({ disposition: "handled" });
  const held = transport.whenHeld();
  expect(await host.submit(input("Reply only HELD.", "prompt", "k3"), "rq-3")).toEqual({ disposition: "started", turn: 2 });
  await within(held, 30_000, "the held response");
  expect(await host.submit(input("Reply only STEERED.", "steer", "k4"), "rq-4")).toEqual({ disposition: "queued" });
  expect(await host.submit(input("Reply only FOLLOWED.", "follow_up", "k5"), "rq-5")).toEqual({ disposition: "queued" });
  expect(await host.submit(input("Reply only BUSY.", "prompt", "k6"), "rq-6")).toEqual({ disposition: "rejected", reason: "busy" });
  expect(host.state().streaming).toBeTrue();
  expect(host.state().turn).toEqual({ current: 2, open: true });
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  expect(await host.submit(input("/fixture-release", "prompt", "k7"), "rq-7")).toEqual({ disposition: "handled" });
  await ended;
  const delivered = (host.state().delivery as { records: { idempotencyKey: string; requestId: string }[] }).records;
  expect(delivered.map((record) => record.idempotencyKey)).toEqual(["k1", "k2", "k3", "k4", "k5", "k6", "k7"]);
});

test("cancel aborts the current turn only", async () => {
  const transport = fixtureTransport();
  expect(await host.cancel(undefined)).toEqual({ cancelled: false, reason: "idle" });
  await host.submit(input("/fixture-hold", "prompt", "k8"), "rq-8");
  const held = transport.whenHeld();
  expect(await host.submit(input("Reply only CANCEL.", "prompt", "k9"), "rq-9")).toEqual({ disposition: "started", turn: 3 });
  await within(held, 30_000, "the held response");
  expect(await host.cancel(99)).toEqual({ cancelled: false, reason: "turn_not_current", turn: 3 });
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 3);
  expect(await host.cancel(3)).toEqual({ cancelled: true, turn: 3 });
  expect((await ended).cancelled).toBeTrue();
});

test("events carry generation 1 and a strictly increasing sequence; corral got each lifecycle event once", async () => {
  await host.drained();
  const page = events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER);
  expect(page.events.every((event) => event.generation === 1)).toBeTrue();
  const seqs = page.events.map((event) => event.seq);
  expect(seqs).toEqual([...seqs].sort((a, b) => a - b));
  expect(new Set(seqs).size).toBe(seqs.length);
  const turns = page.events.filter((event) => ["turn_started", "turn_ended"].includes(event.kind));
  const ids = corral.calls().filter((call) => call.envelope.op === "event").map((call) => call.envelope.request_id);
  expect(ids).toEqual(turns.map((event) => `ev-1-${event.seq}`));
});

test("input held before upstream preflight is never reported rejected, and its turn keeps the bridge origin", async () => {
  const held = holdNextInput();
  let reported: unknown;
  const submitted = host.submit(input("HOLD_INPUT Reply only LATE.", "prompt", "k-late"), "rq-late");
  void submitted.then((result) => { reported = result; });
  await within(held.entered, 30_000, "upstream to hold the input");
  expect(reported).toBeUndefined();
  const started = nextEvent(events, (event) => event.kind === "turn_started");
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  const turn = (host.state().turn as { current: number }).current + 1;
  held.release();
  expect(await submitted).toEqual({ disposition: "started", turn });
  expect((await started).origin).toEqual({ kind: "bridge", idempotency_key: "k-late" });
  await ended;
  await host.drained();
  const reportedOrigin = corral.calls().map((call) => call.envelope.payload)
    .find((payload) => payload.kind === "turn_started" && payload.turn === turn);
  expect(reportedOrigin?.origin).toEqual({ kind: "bridge", idempotency_key: "k-late" });
});

test("through the bridge, input still before preflight at the reply deadline is pending, then settles started with its origin", async () => {
  const server = await startBridge({
    path: join(work, "e2e.sock"), ctlCap: CTL, generation: 1, uid: process.getuid?.() ?? -1, peer: await loadPeerLookup(),
    events, inputReplyMs: 50,
    handlers: {
      hello: () => ({}), state: () => ({}), cancel: async () => ({}), shutdown: () => undefined,
      submit: (submission, requestId) => host.submit(submission, requestId),
    },
  });
  const client = await connect(join(work, "e2e.sock"));
  const held = holdNextInput();
  const args = { text: "HOLD_INPUT Reply only DEFERRED.", busy_policy: "prompt", idempotency_key: "k-deferred", origin: "test" };
  const frame = { version: 1, id: "rq-deferred", ctl_cap: CTL, generation: 1, command: "submit_input", args };
  const first = client.request(frame);
  await within(held.entered, 30_000, "upstream to hold the input");
  const reply = (await first).result;
  expect([reply?.disposition, reply?.reason]).toEqual(["pending", "preflight_pending"]);
  const settled = nextEvent(events, (event) => event.kind === "input_settled" && event.idempotency_key === "k-deferred");
  const started = nextEvent(events, (event) => event.kind === "turn_started");
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  held.release();
  const outcome = await settled;
  expect(outcome.disposition).toBe("started");
  expect((await started).origin).toEqual({ kind: "bridge", idempotency_key: "k-deferred" });
  expect((await started).turn).toBe(outcome.turn);
  await ended;
  const replay = (await client.request({ ...frame, id: "rq-deferred-2" })).result;
  expect([replay?.disposition, replay?.turn, replay?.replayed]).toEqual(["started", outcome.turn, true]);
  expect(events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
    .filter((event) => event.idempotency_key === "k-deferred" && event.disposition === "rejected")).toHaveLength(0);
  client.close();
  await server.close();
});

async function expectRestricted(): Promise<void> {
  const state = host.state();
  expect([...(state.active_tools as string[])].sort()).toEqual(["corral_receipt_lookup", "read"]);
  const resources = state.resources as { extensions: string[]; skills: number; prompts: number; context_files: number };
  expect(resources.skills).toBe(0);
  expect(resources.prompts).toBe(0);
  expect(resources.context_files).toBe(0);
  expect(resources.extensions).toHaveLength(1);
  for (const builtin of BUILTIN_EXTENSIONS) expect(resources.extensions.join(" ")).not.toContain(builtin);
  expect(state.settings).toEqual({
    cacheWarming: "off", enableInstallTelemetry: false, enableAnalytics: false, enableSkillCommands: false,
    quietStartup: true, transport: "sse", extensions: [], compaction: { enabled: true },
  });
  for (const tool of ["fixture_forbidden", "bash"]) {
    const tag = `${tool}-${host.state().last_seq}`;
    await host.submit(input(`/fixture-call ${tool}`, "prompt", `call-${tag}`), "rq-call");
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await host.submit(input("Reply only CALL.", "prompt", `run-${tag}`), "rq-run")).disposition).toBe("started");
    await ended;
    const result = host.session.messages.findLast((message) => message.role === "toolResult") as
      { toolName: string; isError: boolean; content: { text?: string }[] } | undefined;
    expect(result?.toolName).toBe(tool);
    expect(result?.isError).toBeTrue();
    expect(result?.content[0]?.text).toBe(`Tool ${tool} not found`);
  }
  expect(forbiddenRuns()).toBe(0);
  let refused: unknown;
  try {
    await fetch("https://pi.dev/api/report-install?version=1.3.0");
  } catch (error) {
    refused = error;
  }
  expect(refused instanceof NetworkRefused).toBeTrue();
}

test("only the role loadout is active, before and after reload", async () => {
  await expectRestricted();
  expect((await host.submit(input("/fixture-reload", "prompt", "reload"), "rq-reload")).disposition).toBe("handled");
  await expectRestricted();
  const network = host.state().network as { refused: number; allowed: number };
  expect(network.refused).toBe(2);
  expect(network.allowed).toBeGreaterThan(0);
}, 60_000);

test("while bound, every native session replacement is refused and later delivery and tool calls stay in the bound file", async () => {
  // Given the bound host and the session file its hello reported to corral:
  const boundFile = String(corral.calls()[0]?.envelope.payload.session_file);
  expect(host.session.sessionFile).toBe(boundFile);
  // When the owner attempts /new, /resume, /fork and import:
  await expectReplacementsRefused(host, events);
  // Then a delivery and a tool call made afterwards are written to the bound file, after its marker.
  await host.submit(input("/fixture-call read", "prompt", "call-after"), "rq-call-after");
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  expect((await host.submit(input("Reply only AFTER.", "prompt", "after-replace"), "rq-after")).disposition).toBe("started");
  await ended;
  const entries = readFileSync(boundFile, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  const marker = entries.findIndex((entry) => entry.customType === "corral_host_bound");
  const delivered = entries.findIndex((entry) => entry.customType === "corral_fixture_delivery"
    && entry.data.idempotencyKey === "after-replace");
  const called = entries.findIndex((entry, index) => index > delivered && entry.type === "message"
    && entry.message.role === "assistant"
    && entry.message.content.some((item: { type: string; name?: string }) => item.type === "toolCall" && item.name === "read"));
  expect(marker).toBeGreaterThanOrEqual(0);
  expect(delivered).toBeGreaterThan(marker);
  expect(called).toBeGreaterThan(delivered);
  expect(entries[marker].data.session_id).toBe(host.sessionId());
  expect((host.state().session as { created: number }).created).toBe(1);
}, 60_000);
