// The host is the authority on what it cancelled (T-252 recovery): a real HostSession whose turn is
// held inside an accepted provider response is cancelled through the bridge's cancel handler, and
// its authenticated turn_ended report to corral says `cancelled`; a turn that ends on its own
// reports no such field.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../bridge/events.ts";
import { CorralClient, type CorralReceipt, sha256Hex } from "../tools/corral.ts";
import type { HostIdentity } from "../roles/types.ts";
import { fixtureTransport } from "../../test/fixtures/auth/index.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { nextEvent, within } from "../../test/support/wait.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { HostSession, authContext } from "./session.ts";

const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;

beforeAll(() => {
  assets = buildFixtureInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-cancel-report-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("a cancelled turn's turn_ended report says cancelled, and a turn that ends on its own does not", async () => {
  // Given a bound worker host:
  const modules = await loadModules(assets.dir, "worker");
  mkdirSync(join(work, "worktree"), { recursive: true });
  const identity = {
    store: "st-0123456789abcdef", project: "demo", role: "worker", generation: 1, task: "T-001", run: "R1",
  } as ConstructorParameters<typeof CorralClient>[0]["identity"];
  const events = new EventLog(1);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport, () => undefined);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(work);
  let host: HostSession | undefined;
  const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host!.sessionId() });
  host = await HostSession.create({
    sdk, identity, cwd: join(work, "worktree"), agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client,
    events, network, sessionManager: sdk.SessionManager.create(join(work, "worktree"), join(work, "sessions")),
  });
  await host.hello("n".repeat(64), "pane-worker", sha256Hex("c".repeat(64)));
  const submit = (text: string, key: string) =>
    host!.submit({ text, busyPolicy: "follow_up", idempotencyKey: key, origin: "test" }, `rq-${key}`);

  // When one turn ends on its own and the next is cancelled inside its accepted provider response:
  const first = nextEvent(events, (event) => event.kind === "turn_ended");
  expect((await submit("Reply only FIRST.", "k1")).disposition).toBe("started");
  await first;
  const held = fixtureTransport().whenHeld();
  fixtureTransport().holdNext();
  expect((await submit("Reply only HELD.", "k2")).disposition).toBe("started");
  await held;
  const second = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  expect(await host.cancel(undefined)).toEqual({ cancelled: true, turn: 2 });
  await second;
  await host.drained();

  // Then only the cancelled turn's authenticated turn_ended report carries `cancelled`:
  const ended = corral.calls().map((call) => call.envelope)
    .filter((envelope) => envelope.op === "event" && envelope.payload.kind === "turn_ended")
    .map((envelope) => envelope.payload);
  expect(ended).toEqual([{ kind: "turn_ended", turn: 1 }, { kind: "turn_ended", turn: 2, cancelled: true }]);
}, 60_000);

test("a committed hello can await acknowledgment while the host names its matching unbound session", async () => {
  // Given a real HostSession with a committed receipt held at its client boundary:
  const modules = await loadModules(assets.dir, "worker");
  const cwd = join(work, "hello-worktree");
  mkdirSync(cwd);
  const identity = {
    store: "st-0123456789abcdef", project: "demo", role: "worker", generation: 2, task: "T-001", run: "R1",
  } satisfies HostIdentity;
  const events = new EventLog(2);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport, () => undefined);
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(work);
  const committed = Promise.withResolvers<CorralReceipt>();
  const release = Promise.withResolvers<void>();
  let host: HostSession | undefined;
  const client = new CorralClient({
    corral: corral.path, root: work, identity, hostCap: "h".repeat(64),
    sessionId: () => {
      if (!host) throw new Error("the host exists before hello");
      return host.sessionId();
    },
  });
  const call = client.call.bind(client);
  client.call = async (op, args) => {
    const receipt = await call(op, args);
    if (op === "hello") {
      committed.resolve(receipt);
      await release.promise;
    }
    return receipt;
  };
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "hello-agent"), model: "gpt-6.1-sol",
    modules, client, events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "hello-sessions")),
  });

  // When the committed acknowledgment has not reached HostSession.hello yet:
  const hello = host.hello("n".repeat(64), "pane-worker", sha256Hex("c".repeat(64)));
  try {
    expect((await within(committed.promise, 30_000, "the committed hello receipt")).state).toBe("committed");

    // Then get_state reports the exact legitimate state liveness must not call another host:
    const state = host.state();
    expect(state.generation).toBe(2);
    expect(state.bound).toBeFalse();
    const session = state.session;
    if (typeof session !== "object" || session === null) throw new Error("get_state names its session");
    expect(Reflect.get(session, "bound_id")).toBeNull();
    expect(Reflect.get(session, "current_id")).toBe(host.sessionId());
    expect(events.since(0, 64 * 1024).events.some((event) => event.kind === "bound")).toBeFalse();
  } finally {
    const bound = nextEvent(events, (event) => event.kind === "bound");
    release.resolve();
    await within(hello, 30_000, "the released hello acknowledgment");
    await bound;
  }
  expect(host.state().bound).toBeTrue();
}, 60_000);
