// T-605 in a real HostSession: the production worker role's turns, corral_ask and corral_withdraw
// receipts and an inbox answer drive the one herdr adapter, recorded at its herdr boundary; the
// hello receipt binds it and a graceful close releases it last.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { HerdrLifecycle, SEQ_SHIFT } from "../src/host/herdr.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const EPOCH = 1_793_000_000_123;
const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;

beforeAll(() => {
  assets = buildFixtureInstall();
  for (const tree of ["src/roles/worker", "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  work = mkdtempSync(join(tmpdir(), "corral-pi-herdr-host-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("turns, question receipts and an inbox answer drive the herdr projection, and close releases it", async () => {
  // Given a bound production worker host whose herdr calls are recorded:
  const identity: HostIdentity = { store: "st-demo", project: "demo", role: "worker", task: "T-001", run: "R1", generation: 1 };
  const modules = await loadModules(assets.dir, "worker");
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const events = new EventLog(1);
  const corral = writeFakeCorral(work);
  const calls: string[][] = [];
  const herdr = new HerdrLifecycle({ bin: "/opt/herdr", pane: "w1:p4" }, identity,
    (kind, data) => events.emit(kind, data), async (_bin, args) => { calls.push([...args]); });
  let host: HostSession | undefined;
  const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64),
    sessionId: () => host?.sessionId() ?? "" });
  const call = client.call.bind(client);
  client.call = async (op, args) => {
    const receipt = await call(op, args);
    return op === "hello" ? { ...receipt, result: { ...receipt.result as object, herdr_seq_epoch_ms: EPOCH, questions: [] } } : receipt;
  };
  const cwd = join(work, "cwd");
  mkdirSync(cwd);
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules, client, events, network, herdr,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({});
  const states = () => calls.filter((c) => c[1] === "report-agent").map((c) => c[c.indexOf("--state") + 1]);
  const submit = async (text: string, key: string, origin = "test") => {
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await host!.submit({ text, busyPolicy: "prompt", idempotencyKey: key, origin }, `rq-${key}`)).disposition)
      .toBe("started");
    await ended;
  };
  const idle = nextEvent(events, (event) => event.kind === "herdr_reported" && event.state === "idle");
  await host.hello("n".repeat(64), "w1:p4", sha256Hex("c".repeat(64)));
  await idle;
  expect(calls[0]?.slice(0, 2)).toEqual(["pane", "report-metadata"]);
  expect(calls[1]).toContain(host.sessionId());

  // When a turn asks the owner a question through corral_ask and ends:
  fixtureTransport().script([
    { toolCalls: [{ callId: "ask-1", name: "corral_ask", arguments: { kind: "owner_only", text: "Ship it?", blocking: true } }] },
    { text: "Waiting for the owner." },
  ]);
  await submit("Ask the owner.", "ask");

  // Then the projection blocks and stays blocked after the turn settles:
  expect(states()).toEqual(["idle", "working", "blocked"]);
  expect((host.state().herdr as { blocked_on: string[] }).blocked_on).toEqual(["T-001/R1/Q1"]);

  // When corral delivers the owner's answer through the inbox:
  await submit("[corral envelope T-001_R1@env1]\n\n[corral T-001_R1#1 question_answer]\n"
    + "Answer to T-001/R1/Q1 (revision 2) by Owner:\nYes.", "answer", "corral-inbox");

  // Then the question no longer blocks: the answer's turn worked and settled idle.
  const answered = states().slice(3);
  expect(answered.at(-1)).toBe("idle");
  expect(answered).toContain("working");
  expect(answered).not.toContain("blocked");

  // When one turn asks a blocking question and withdraws it:
  const before = states().length;
  fixtureTransport().script([
    { toolCalls: [{ callId: "ask-2", name: "corral_ask", arguments: { kind: "clarification", text: "Which?", blocking: true } }] },
    { toolCalls: [{ callId: "withdraw-2", name: "corral_withdraw", arguments: { key: "T-001/R1/Q2", reason: "Found it." } }] },
    { text: "Done." },
  ]);
  await submit("Ask, then withdraw.", "withdraw");

  // Then it blocked while the question was pending and ended idle:
  expect(states().slice(before)).toEqual(["working", "blocked", "working", "idle"]);

  // When corral reports that the PM escalated a non-blocking question, then that the owner
  // cancelled it (bridge `questions`), while the host is idle:
  const external = states().length;
  const released = nextEvent(events, (event) => event.kind === "herdr_reported" && event.state === "idle"
    && states().length > external);
  expect(host.questionsChanged([{ key: "T-001/R1/Q9", revision: 2, kind: "clarification", blocking: false,
    state: "escalated" }])).toEqual({ applied: true, blocked_on: ["T-001/R1/Q9"] });
  host.questionsChanged([{ key: "T-001/R1/Q9", revision: 3, kind: "clarification", blocking: false,
    state: "cancelled" }]);
  await released;

  // Then the projection blocked on the escalation and released on the cancellation:
  expect(states().slice(external)).toEqual(["blocked", "idle"]);

  // When the host closes gracefully:
  await host.close();

  // Then its last herdr calls release the pane and clear its metadata, in sequence order:
  expect(calls.at(-2)?.slice(0, 2)).toEqual(["pane", "release-agent"]);
  expect(calls.at(-1)).toContain("--clear-display-agent");
  const seqs = calls.map((c) => BigInt(c[c.indexOf("--seq") + 1] ?? "0"));
  expect(seqs[0]).toBe((BigInt(EPOCH) << SEQ_SHIFT) + 1n);
  expect(seqs.every((seq, i) => i === 0 || seq > (seqs[i - 1] ?? 0n))).toBeTrue();
}, 60_000);
