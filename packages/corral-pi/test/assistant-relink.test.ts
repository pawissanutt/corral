import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { openSession } from "../src/host/lifecycle.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent, within } from "./support/wait.ts";

test("a changed scratch link restarts only at idle and resumes the same chat automatically", async () => {
  // Given a supervised assistant and a provider response held in the middle of a turn.
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-assistant-relink-"));
  const project = join(work, "app");
  const scratch = join(project, ".worktrees/A-1-linked");
  mkdirSync(scratch, { recursive: true });
  const sdk = await import("@earendil-works/pi-coding-agent");
  const modules = await loadModules(assets.dir, "assistant");
  const send = modules.auth.transport?.(authContext());
  if (!send) throw new Error("fixture transport missing");
  const fake = writeFakeCorral(work);
  const identity = { store: "st-0123456789abcdef", project: "app", role: "assistant" as const, assistant: "A-1", generation: 1 };
  const events = new EventLog(1);
  const sessions = join(work, "sessions");
  let requests = 0;
  let host: HostSession | undefined;
  let resumed: HostSession | undefined;
  try {
    fake.snapshot({ session: { id: "A-1", ended: null }, directory: { path: project, branch: "main" } });
    const client = new CorralClient({ corral: fake.path, root: work, identity,
      hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({
      sdk, identity, cwd: project, root: work, agentDir: join(work, "agent-1"),
      model: "gpt-6.1-sol", modules, ui: "plain", client, events, network: guardedFetch(send),
      sessionManager: sdk.SessionManager.create(project, sessions),
      relinkAssistant: () => { requests++; },
    });
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    await host.session.bindExtensions({});
    const transport = fixtureTransport();
    transport.holdNext();
    const held = transport.whenHeld();
    const turn = host.session.prompt("Finish the fixture work.", { source: "interactive" });
    await within(held, 10_000, "held provider response");
    // When a scratch link changes while the turn is still open.
    fake.snapshot({ session: { id: "A-1", ended: null, linked_scratch: "linked" },
      directory: { path: scratch, branch: "corral/A-1/linked" } });
    expect(requests).toBe(0);
    expect(host.currentTurn()).toBe(1);
    transport.release();
    await turn;
    await host.drained();
    // Then exactly one relink is requested after settlement, without replacing the Pi file.
    expect(requests).toBe(1);
    const file = host.session.sessionFile;
    const sessionId = host.sessionId();
    if (!file) throw new Error("the first turn must persist its session");
    expect(await host.submit({ text: "late", busyPolicy: "prompt", idempotencyKey: "late", origin: "test" }, "rq-late"))
      .toEqual({ disposition: "rejected", reason: "host_closing" });
    await host.close();
    host = undefined;
    const nextIdentity = { ...identity, generation: 2 };
    const nextEvents = new EventLog(2);
    const manager = openSession(sdk, scratch, sessions, 2, file);
    resumed = await HostSession.create({
      sdk, identity: nextIdentity, cwd: scratch, root: work, agentDir: join(work, "agent-2"),
      model: "gpt-6.1-sol", modules, ui: "plain",
      client: new CorralClient({ corral: fake.path, root: work, identity: nextIdentity,
        hostCap: "j".repeat(64), sessionId: () => resumed?.sessionId() ?? "" }),
      events: nextEvents, network: guardedFetch(send), sessionManager: manager,
      relinkAssistant: () => { requests++; },
    });
    await resumed.hello("m".repeat(64), "pane-1", sha256Hex("d".repeat(64)));
    const continued = nextEvent(nextEvents, event => event.kind === "turn_ended");
    await resumed.session.bindExtensions({});
    await continued;
    await resumed.drained();
    expect(resumed.sessionId()).toBe(sessionId);
    expect(resumed.session.sessionFile).toBe(file);
    expect(manager.getCwd()).toBe(scratch);
    expect(requests).toBe(1);
    expect(fake.calls().filter(call => call.envelope.payload.kind === "owner_input")).toHaveLength(1);
    const starts = fake.calls().filter(call => call.envelope.payload.kind === "turn_started");
    expect(starts.map(call => call.envelope.identity.generation)).toEqual([1, 2]);
    expect(starts[1]?.envelope.payload.origin).not.toEqual({ kind: "owner_terminal" });
    // Reattaching extensions does not deliver the relink prompt again.
    const count = transport.requests;
    await resumed.session.bindExtensions({});
    await resumed.session.waitForIdle();
    expect(transport.requests).toBe(count);
  } finally {
    await host?.close();
    await resumed?.close();
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
});
