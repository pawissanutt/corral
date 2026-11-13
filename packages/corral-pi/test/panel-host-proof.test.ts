import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type BridgeEvent, EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { forbiddenRuns } from "./fixtures/roles/fixture-role.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";
import { nextEvent, within } from "./support/wait.ts";

// T-687 layout proof: the production seam (PanelTerminal + mainView + extension UI) over a real
// HostSession, InteractiveMode and editor. Only provider and control-plane I/O are fixtures.
const RAIL_ORIGIN = "\x1b[1;107H"; // 140 columns: 105 main + 1 gap, rail painted from column 107.

test("panel layout streams a held reply and tool output, keeps one binding through inspection and resize, submits once and refuses", async () => {
  const work = mkdtempSync(join(tmpdir(), "corral-panel-proof-")), assets = buildFixtureInstall();
  const oldFetch = globalThis.fetch, oldState = process.env.PI_CODING_AGENT_DIR;
  let host: HostSession | undefined;
  let mode: InstanceType<(typeof import("@earendil-works/pi-coding-agent"))["InteractiveMode"]> | undefined;
  let release: () => void = () => {};
  try {
    restrictEnvironment(process.env); process.env.PI_CODING_AGENT_DIR = join(work, "pi-state");
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
    const modules = await loadModules(assets.dir, "worker");
    const send = modules.auth.transport?.(authContext());
    if (!send) throw new Error("fixture transport unavailable");
    const events = new EventLog(1), network = guardedFetch(send);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const corral = writeFakeCorral(work);
    const client = new CorralClient({ corral: corral.path, root: work, identity,
      hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"),
      model: "gpt-6.1-sol", modules, client, events, network,
      sessionManager: sdk.SessionManager.create(work, join(work, "sessions")),
    });
    await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
    const session = host, bound = host.sessionId(), panels = host.panels;
    if (!panels) throw new Error("rich host did not install native panels");
    const seen: BridgeEvent[] = [];
    events.on(event => { seen.push(event); });
    const count = (kind: string) => seen.filter(event => event.kind === kind).length;
    const binding = () => {
      const state = session.state() as { generation: number; bound: boolean; session: { bound_id: string; current_id: string; created: number };
        turn: { current: number; open: boolean } };
      return { generation: state.generation, bound: state.bound, boundId: state.session.bound_id, currentId: state.session.current_id,
        created: state.session.created, turn: state.turn };
    };
    const owner = () => corral.calls().filter(call => call.envelope.payload.kind === "turn_started");
    const type = (text: string) => { for (const char of text) physical.send(char); };

    const physical = new RecordingTerminal(), terminal = panels.wrap(physical);
    mode = new sdk.InteractiveMode(host.runtime, { terminal, tuiMode: "fullscreen", readOnlyQueue: true, verbose: false, mainView: transcript => panels.mainView(transcript) });
    await mode.init(); host.markAttached("interactive");
    void mode.run().catch(error => { throw error; });
    await physical.untilVisible("WORKER ● g1"); // the bottom bar's bound dot
    physical.send("\x1b[20~"); // task-bound sessions open on Task; F9 shows the transcript
    expect(terminal.columns).toBe(105);
    const initial = binding();
    expect(initial).toMatchObject({ generation: 1, bound: true, boundId: bound, currentId: bound, turn: { current: 0, open: false } });

    // One provider turn: a real read tool call, then a reply held after its first streamed delta.
    writeFileSync(join(work, "tool-output.txt"), "TOOL_OUTPUT_SENTINEL\n");
    const held = new Promise<void>(resolve => { release = resolve; });
    let reached: () => void = () => {};
    const firstChunk = new Promise<void>(resolve => { reached = resolve; });
    fixtureTransport().script([
      { toolCalls: [{ callId: "proof-read", name: "read", arguments: { path: join(work, "tool-output.txt") } }] },
      { chunks: ["STREAM_PART_ONE", " STREAM_PART_TWO"], afterFirstChunk: () => { reached(); return held; } },
    ]);
    type("Run the panel proof.");
    await physical.until("Run the panel proof.");
    expect(count("turn_started")).toBe(0);
    let mark = physical.writes.length;
    physical.send("\r");
    await within(firstChunk, 10_000, "the first streamed delta");
    await physical.until("STREAM_PART_ONE", mark);
    await physical.until("tool-output.txt", mark);
    // The partial reply and the rail arrive in the same synchronized frame from the panel terminal.
    const partial = physical.writes.slice(mark).find(write => write.includes("STREAM_PART_ONE"));
    expect(partial).toContain(RAIL_ORIGIN);
    expect(physical.writes.join("")).not.toContain("TOOL_OUTPUT_SENTINEL");
    physical.send("\x0f"); // Stock app.tools.expand reaches Pi through the panel terminal.
    await physical.until("TOOL_OUTPUT_SENTINEL", mark);
    expect(physical.writes.join("")).not.toContain("STREAM_PART_TWO");
    expect(count("turn_started")).toBe(1);
    expect(count("turn_ended")).toBe(0);
    expect(binding().turn).toEqual({ current: 1, open: true });

    // Inspection while the reply is held: Agents, then Chat again.
    mark = physical.writes.length;
    physical.send("\x1b[19~");
    expect(panels.workbench.view).toBe("agents");
    await physical.untilVisible("helper calls and jobs", mark); // the agents view replaces the transcript
    mark = physical.writes.length;
    physical.send("\x1b[20~");
    expect(panels.workbench.view).toBe("conversation");
    await physical.until("STREAM_PART_ONE", mark);
    expect(binding()).toEqual({ ...initial, turn: { current: 1, open: true } });

    // Resize while the reply is held, with an unsent draft in the real editor.
    type("Draft survives");
    for (const [columns, rows, main] of [[80, 34, 80], [52, 24, 52], [140, 46, 105]] as const) {
      mark = physical.writes.length;
      physical.resize(columns, rows);
      await physical.until("Draft survives", mark);
      expect(terminal.columns).toBe(main);
      expect(binding()).toEqual({ ...initial, turn: { current: 1, open: true } });
    }
    expect(physical.writes.join("")).not.toContain("STREAM_PART_TWO");
    expect(count("turn_ended")).toBe(0);
    physical.send("\x15");

    const ended = nextEvent(events, event => event.kind === "turn_ended");
    mark = physical.writes.length;
    release();
    await ended; await host.drained();
    await physical.until("STREAM_PART_TWO", mark);
    // The submission reached the existing host path exactly once: one user message, one turn, one owner report.
    expect(host.session.messages.filter(message => message.role === "user")).toHaveLength(1);
    expect(count("turn_started")).toBe(1);
    expect(owner().map(call => call.envelope.payload.origin)).toEqual([{ kind: "owner_terminal" }]);
    expect(binding()).toEqual({ ...initial, turn: { current: 1, open: false } });

    // The composer cannot bypass the role's execution guard or the bound-session replacement fence.
    const forbidden = forbiddenRuns();
    type("/fixture-call fixture_forbidden"); physical.send("\r");
    const denied = nextEvent(events, event => event.kind === "turn_ended");
    mark = physical.writes.length;
    type("Call the fixture sentinel."); physical.send("\r");
    await denied; await host.drained();
    await physical.until("Tool fixture_forbidden not found", mark);
    expect(forbiddenRuns()).toBe(forbidden);
    const tools = session.state() as { loadout: string[]; active_tools: string[] };
    expect([...tools.loadout, ...tools.active_tools]).not.toContain("fixture_forbidden");
    const refused = nextEvent(events, event => event.kind === "session_replacement_refused");
    type("/fixture-new-session"); physical.send("\r");
    await refused;
    expect(binding()).toEqual({ ...initial, turn: { current: 2, open: false } });
    expect(owner().map(call => call.envelope.payload.origin)).toEqual([{ kind: "owner_terminal" }, { kind: "owner_terminal" }]);
    expect(corral.calls().every(call => call.envelope.identity.generation === 1)).toBeTrue();
  } finally {
    release();
    mode?.stop(); await host?.close(); assets.cleanup(); rmSync(work, { recursive: true, force: true });
    globalThis.fetch = oldFetch;
    if (oldState === undefined) delete process.env.PI_CODING_AGENT_DIR; else process.env.PI_CODING_AGENT_DIR = oldState;
  }
}, 30_000);
