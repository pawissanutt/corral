// The host is the authority on what it cancelled (T-252 recovery): a real HostSession whose turn is
// held inside an accepted provider response is cancelled through the host's cancel handler.
// Only the cancelled turn's local turn_ended event carries cancelled: true.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../bridge/events.ts";
import { CorralClient } from "../tools/corral.ts";
import { fixtureTransport } from "../../test/fixtures/auth/index.ts";
import { writeFakeEngine } from "../../test/support/fake-engine.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { nextEvent } from "../../test/support/wait.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { HostSession, authContext } from "./session.ts";

const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;
let host: HostSession | undefined;

beforeAll(() => {
  assets = buildFixtureInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-cancel-report-"));
  restrictEnvironment(process.env);
});

afterAll(async () => {
  await host?.close();
  await host?.drained();
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("turn_ended identifies only the cancelled turn as cancelled", async () => {
  // Given a bound worker host:
  const modules = await loadModules(assets.dir, "worker");
  mkdirSync(join(work, "worktree"), { recursive: true });
  const identity = {
    store: "st-0123456789abcdef", project: "demo", role: "worker", task: "T-001", run: "R1",
  } as ConstructorParameters<typeof CorralClient>[0]["identity"];
  const events = new EventLog();
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport, () => undefined);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const cwd = join(work, "worktree");
  const client = new CorralClient({ corral: writeFakeEngine(work).path, root: work, cwd, identity });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client,
    events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.startLocal();

  // When one turn ends on its own and the next is cancelled inside its accepted provider response:
  const firstEnded = nextEvent(events, (event) => event.kind === "turn_ended");
  expect((await host.submitLocal({ text: "Reply only FIRST.", busyPolicy: "follow_up", origin: "owner" })).disposition).toBe("started");
  expect(await firstEnded).toMatchObject({ kind: "turn_ended", turn: 1, cancelled: false });
  const held = fixtureTransport().whenHeld();
  fixtureTransport().holdNext();
  expect((await host.submitLocal({ text: "Reply only HELD.", busyPolicy: "follow_up", origin: "owner" })).disposition).toBe("started");
  await held;
  const second = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  expect(await host.cancel(undefined)).toEqual({ cancelled: true, turn: 2 });
  expect(await second).toMatchObject({ kind: "turn_ended", turn: 2, cancelled: true });
}, 60_000);
