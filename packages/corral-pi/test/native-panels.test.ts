import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";
import { nextEvent, within } from "./support/wait.ts";
import { forbiddenRuns } from "./fixtures/roles/fixture-role.ts";

// Real production HostSession + InteractiveMode + editor. Only provider/control-plane I/O is faked.
test("native panels retain real streaming, composing, binding, modal isolation and resize", async () => {
  const work = mkdtempSync(join(tmpdir(), "corral-native-ui-")), assets = buildFixtureInstall();
  const oldFetch = globalThis.fetch, oldState = process.env.PI_CODING_AGENT_DIR;
  let host: HostSession | undefined;
  let mode: InstanceType<(typeof import("@earendil-works/pi-coding-agent"))["InteractiveMode"]> | undefined;
  try {
    restrictEnvironment(process.env); process.env.PI_CODING_AGENT_DIR = join(work, "pi-state");
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const,
      task: "T-001", run: "R1", generation: 1 };
    const originalModules = await loadModules(assets.dir, "worker");
    let answer: boolean | undefined;
    const modules = { ...originalModules, role: { ...originalModules.role,
      extend(pi: Parameters<NonNullable<typeof originalModules.role.extend>>[0], context: Parameters<NonNullable<typeof originalModules.role.extend>>[1]) {
        originalModules.role.extend?.(pi, context);
        pi.registerCommand("native-confirm", { description: "Owned fixture confirmation", handler: async (_args, ctx) => {
          answer = await ctx.ui.confirm("Native confirmation", "Cancel this owned fixture prompt.");
        } });
      },
    } };
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport unavailable");
    const events = new EventLog(1), network = guardedFetch(transport);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const corral = writeFakeCorral(work);
    const client = new CorralClient({ corral: corral.path, root: work, identity,
      hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"),
      model: "gpt-6.1-sol", modules, client, events, network,
      sessionManager: sdk.SessionManager.create(work, join(work, "sessions")),
    });
    await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
    const bound = host.sessionId(), panels = host.panels;
    if (!panels) throw new Error("rich host did not install native panels");
    const physical = new RecordingTerminal(), terminal = panels.wrap(physical);
    mode = new sdk.InteractiveMode(host.runtime, { terminal, tuiMode: "fullscreen", verbose: false });
    await mode.init(); host.markAttached("interactive");
    const running = mode.run();
    // Stop is explicit below; no Ctrl+D/process.exit is sent to the test runner.
    void running.catch(error => { throw error; });
    await physical.until("Bound to Corral");
    expect(terminal.columns).toBe(105);
    const firstEnd = nextEvent(events, event => event.kind === "turn_ended");
    for (const char of "Reply only NATIVE_PANEL_OK.") physical.send(char);
    physical.send("\r"); await firstEnd; await host.drained();
    await physical.until("NATIVE_PANEL_OK");
    expect(host.session.messages.some(message => message.role === "user")).toBeTrue();
    expect(corral.calls().some(call => call.envelope.payload.kind === "turn_started"
      && JSON.stringify(call.envelope.payload.origin) === JSON.stringify({ kind: "owner_terminal" }))).toBeTrue();
    expect(host.sessionId()).toBe(bound);
    const evidence = process.env.CORRAL_NATIVE_PANEL_EVIDENCE;
    if (evidence) { mkdirSync(evidence, { recursive: true }); writeFileSync(join(evidence, "wide.ansi"), physical.writes.join("")); }

    for (const char of "Keep this unsent draft") physical.send(char);
    let mark = physical.writes.length;
    physical.resize(80, 34); await physical.until("F6 inspect", mark);
    expect(terminal.columns).toBe(80); expect(host.sessionId()).toBe(bound);
    physical.send("\x1b[17~"); physical.send("\x1b[C");
    expect(panels.tab).toBe("Files"); physical.send("\x1b");
    mark = physical.writes.length;
    physical.send("\x1b[17~"); physical.send("\x1b[D");
    await physical.until("↑↓ Enter", mark);
    expect(panels.focused).toBeTrue();
    physical.send("\x1b[<0;6;32M"); physical.send("\x1b[<0;6;32m");
    expect(panels.focused).toBeFalse(); // Real editor click releases logical inspector focus.
    physical.send("\x1b[F"); // Keep the fake-cursor escape outside the draft substring checked on resize.
    if (evidence) writeFileSync(join(evidence, "narrow.ansi"), physical.writes.join(""));
    mark = physical.writes.length;
    physical.resize(52, 24); await physical.until("F6 inspect", mark);
    expect(terminal.columns).toBe(52);
    if (evidence) writeFileSync(join(evidence, "small.ansi"), physical.writes.join(""));
    physical.resize(140, 46); await physical.until("Keep this unsent draft", mark);
    expect(terminal.columns).toBe(105);

    panels.tab = "Context";
    const confirmed = host.session.prompt("/native-confirm", { source: "interactive" });
    await physical.until("Native confirmation");
    physical.send("\x02");
    physical.send("\x1b[<0;117;2M"); physical.send("\x1b[<0;117;2m");
    expect(terminal.columns).toBe(105); expect(panels.tab).toBe("Context");
    physical.send("\x1b"); await within(confirmed, 10_000, "native cancel");
    expect(answer).toBeFalse();

    // The presentation cannot bypass the existing role's tool execution guard.
    const forbidden = forbiddenRuns();
    await host.session.prompt("/fixture-call fixture_forbidden", { source: "interactive" });
    const deniedEnd = nextEvent(events, event => event.kind === "turn_ended");
    await host.session.prompt("Call the fixture sentinel.", { source: "interactive" });
    await deniedEnd; await host.drained();
    expect(forbiddenRuns()).toBe(forbidden);
    expect(host.sessionId()).toBe(bound);
    if (evidence) writeFileSync(join(evidence, "result.json"), JSON.stringify({ status: "PASS", bound,
      checks: ["real InteractiveMode", "real editor submission", "streamed fake-provider response", "bound identity retained",
        "140→80→52→140 resize", "unsent draft retained", "native inspector keyboard", "modal background isolation", "role guard retained"],
      backend: "deterministic fixture provider and fake control plane; no network or live workspace mutations" }, null, 2));
  } finally {
    mode?.stop(); await host?.close(); assets.cleanup(); rmSync(work, { recursive: true, force: true });
    globalThis.fetch = oldFetch;
    if (oldState === undefined) delete process.env.PI_CODING_AGENT_DIR; else process.env.PI_CODING_AGENT_DIR = oldState;
  }
}, 30_000);
