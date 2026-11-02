// A managed assistant requests a supervisor transition without replacing its bound Pi runtime.
import { expect, test } from "bun:test";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";

test("managed /new hands off once; RPC and native replacement cannot swap the bound file", async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-assistant-new-"));
  let host: HostSession | undefined;
  let mode: InstanceType<(typeof import("@earendil-works/pi-coding-agent"))["InteractiveMode"]> | undefined;
  let requests = 0;
  try {
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "", role: "assistant" as const, assistant: "A-1", generation: 1 };
    const modules = await loadModules(assets.dir, "assistant");
    const send = modules.auth.transport?.(authContext());
    if (!send) throw new Error("fixture auth must supply its offline transport");
    const fake = writeFakeCorral(work);
    const events = new EventLog(1);
    const sessions = join(work, "sessions");
    host = await HostSession.create({
      sdk, identity, cwd: work, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, ui: "plain",
      client: new CorralClient({ corral: fake.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" }),
      events, network: guardedFetch(send), sessionManager: sdk.SessionManager.create(work, sessions),
      newAssistant: () => { requests++; },
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    // A non-interactive runtime cannot use the interactive supervisor handoff.
    expect((await host.runtime.newSession()).cancelled).toBeTrue();
    expect(requests).toBe(0);
    await host.session.bindExtensions({ mode: "tui" });
    const original = host.session;
    const file = original.sessionFile;
    // Bridge input does not run the native /new UI command or grant lifecycle authority.
    await host.submit({ text: "/new", busyPolicy: "prompt", idempotencyKey: "remote-new", origin: "test" }, "rq-remote");
    await host.session.waitForIdle();
    expect(requests).toBe(0);
    // A busy assistant cannot drop work; after it settles the same command can succeed.
    await host.submit({ text: "/fixture-hold", busyPolicy: "prompt", idempotencyKey: "hold", origin: "test" }, "rq-hold");
    // turn_started precedes provider I/O. Wait for the held response itself so cancellation
    // consumes fixture-hold instead of leaking it into the next host's provider request.
    const held = fixtureTransport().whenHeld();
    await host.submit({ text: "Reply only HELD.", busyPolicy: "prompt", idempotencyKey: "busy", origin: "test" }, "rq-busy");
    await held;
    expect((await host.runtime.newSession()).cancelled).toBeTrue();
    expect(requests).toBe(0);
    const ended = nextEvent(events, event => event.kind === "turn_ended");
    await host.cancel(undefined);
    await ended;
    await host.drained();
    // Native /new cancels Pi's replacement but requests exactly one Corral transition.
    const files = readdirSync(sessions);
    const physical = new RecordingTerminal();
    mode = new sdk.InteractiveMode(host.runtime, { terminal: physical, verbose: false });
    await mode.init();
    void mode.run();
    const requested = nextEvent(events, event => event.kind === "assistant_new_requested");
    for (const char of "/new") physical.send(char);
    physical.send("\r");
    await requested;
    if (process.env.CORRAL_ASSISTANT_NEW_EVIDENCE) {
      writeFileSync(process.env.CORRAL_ASSISTANT_NEW_EVIDENCE, physical.writes.join(""));
    }
    expect((await host.runtime.newSession()).cancelled).toBeTrue();
    expect(requests).toBe(1);
    expect(host.session).toBe(original);
    expect(host.session.sessionFile).toBe(file);
    expect(readdirSync(sessions)).toEqual(files);
    expect(await host.submit({ text: "late", busyPolicy: "prompt", idempotencyKey: "late", origin: "test" }, "rq-late"))
      .toEqual({ disposition: "rejected", reason: "host_closing" });
    expect(events.since(0, 1_000_000, 1000).events.filter(event => event.kind === "assistant_new_requested")).toHaveLength(1);
  } finally {
    mode?.stop();
    await host?.close();
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
});
