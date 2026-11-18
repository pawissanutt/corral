// A local worker binding projects turns and owner questions to herdr. Reload must not release that pane;
// disposing the runtime must release it before an outer cleanup runs.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { HerdrLifecycle } from "../src/host/herdr.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { nextEvent } from "./support/wait.ts";

const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;
let host: HostSession | undefined;

beforeAll(() => {
  assets = buildFixtureInstall();
  for (const tree of ["src/roles/worker", "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  work = mkdtempSync(join(tmpdir(), "corral-pi-herdr-host-"));
  restrictEnvironment(process.env);
});

afterAll(async () => {
  await host?.close();
  await host?.drained();
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("a local binding projects turns and owner questions to herdr and releases the pane on shutdown, not reload", async () => {
  const identity: HostIdentity = { store: "st-demo", project: "demo", role: "worker", task: "T-001", run: "R1" };
  const modules = await loadModules(assets.dir, "worker");
  const provider = fakeCodexTransport();
  const network = guardedFetch(provider.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const events = new EventLog();
  const corral = writeFakeEngine(work);
  const calls: string[][] = [];
  const herdr = new HerdrLifecycle({ bin: "/opt/herdr", pane: "w1:p4" }, identity,
    (kind, data) => events.emit(kind, data), async (_bin, args) => { calls.push([...args]); });
  const cwd = join(work, "cwd");
  mkdirSync(cwd);
  const client = new CorralClient({ corral: corral.path, root: work, cwd, identity });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules, client, events, network, herdr,
    capabilities: { catalog: 7, enabled: ["ask_user"] },
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({ mode: "print" });
  const states = () => calls.filter((c) => c[1] === "report-agent").map((c) => c[c.indexOf("--state") + 1]);
  const idle = nextEvent(events, (event) => event.kind === "herdr_reported" && event.state === "idle");
  await host.startLocal();
  await idle;
  expect(calls[0]?.slice(0, 2)).toEqual(["pane", "report-metadata"]);
  expect(calls[1]).toContain(host.sessionId());
  const file = host.session.sessionFile;
  if (!file) throw new Error("missing locally bound transcript");
  expect(calls[1]).toContain(file);

  provider.script([
    { toolCalls: [{ callId: "ask", name: "ask_user", arguments: {
      nonBlocking: true, questions: [{ question: "Ship it?", options: ["Yes", "No"] }],
    } }] },
    { text: "Waiting for the owner." },
  ]);
  const blocked = nextEvent(events, event => event.kind === "herdr_reported" && event.state === "blocked");
  const ended = nextEvent(events, event => event.kind === "turn_ended");
  expect((await host.submitLocal({ text: "Ask the owner.", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
  await blocked;
  await ended;
  expect(states()).toEqual(["idle", "working", "blocked"]);
  const [question] = host.pendingOwnerQuestions();
  if (!question) throw new Error("the turn did not create an owner question");
  expect(herdr.readback().blocked_on).toEqual([question.id]);

  const held = provider.whenHeld();
  provider.holdNext();
  const working = nextEvent(events, event => event.kind === "herdr_reported" && event.state === "working");
  await host.answerQuestion({ question: question.id, option: "Yes" }, async () => undefined);
  await held;
  await working;
  const settled = nextEvent(events, event => event.kind === "herdr_reported" && event.state === "idle");
  provider.release();
  await settled;
  await host.drained();
  expect(states().at(-1)).toBe("idle");
  expect(herdr.readback().blocked_on).toEqual([]);

  // Reload also emits session_shutdown, but must keep the bound pane's projection alive.
  await host.session.reload();
  expect(calls.some((c) => c[1] === "release-agent")).toBeFalse();
  expect(herdr.readback().closed).toBeFalse();

  // InteractiveMode quits by disposing the runtime, then process.exit(), without returning
  // to main's finally block. Exercise that shutdown path without calling host.close().
  await host.runtime.dispose();

  // Then its last herdr calls release the pane and clear its metadata, in sequence order:
  expect(calls.at(-2)?.slice(0, 2)).toEqual(["pane", "release-agent"]);
  expect(calls.at(-1)).toContain("--clear-display-agent");
  const seqs = calls.map((c) => BigInt(c[c.indexOf("--seq") + 1] ?? "0"));
  expect(seqs.every((seq, i) => i === 0 || seq > (seqs[i - 1] ?? 0n))).toBeTrue();

  // A later outer cleanup must not release twice.
  const closedCalls = calls.length;
  await host.close();
  expect(calls).toHaveLength(closedCalls);
}, 60_000);
