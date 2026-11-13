import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ScrollView, type Component } from "@earendil-works/pi-tui";
import { EventLog } from "../../src/bridge/events.ts";
import { loadModules } from "../../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../src/host/restrict.ts";
import { HostSession, authContext } from "../../src/host/session.ts";
import { CorralClient, sha256Hex } from "../../src/tools/corral.ts";
import type { CapabilitySet } from "../../src/roles/types.ts";
import type { NativePanels } from "../../src/ui/panels.ts";
import { buildFixtureInstall } from "./fixture-install.ts";
import { writeFakeCorral } from "./fake-corral.ts";
import { RecordingTerminal } from "./panel-terminal.ts";
import { ScreenModel } from "./screen.ts";

type Sdk = typeof import("@earendil-works/pi-coding-agent");
type Modules = Awaited<ReturnType<typeof loadModules>>;
type Extend = NonNullable<Modules["role"]["extend"]>;

export interface PanelHost {
  readonly work: string;
  readonly host: HostSession;
  readonly panels: NativePanels;
  readonly physical: RecordingTerminal;
  readonly events: EventLog;
  readonly bound: string;
  /** The SDK's own transcript ScrollView, captured where Pi hands it to the mainView hook. */
  readonly transcript: ScrollView;
  readonly chatPresentation: () => import("@earendil-works/pi-coding-agent").InteractiveChatPresentation;
  screen(): ScreenModel;
  resize(columns: number, rows: number): void;
  stop(): Promise<void>;
}

/** Real HostSession + InteractiveMode + editor through PanelTerminal; only provider/control-plane I/O is faked. */
export async function startPanelHost(extend?: (...args: Parameters<Extend>) => void,
  options: { readonly capabilities?: CapabilitySet } = {}): Promise<PanelHost> {
  const work = mkdtempSync(join(tmpdir(), "corral-panel-shell-")), assets = buildFixtureInstall();
  const oldFetch = globalThis.fetch, oldState = process.env.PI_CODING_AGENT_DIR;
  let host: HostSession | undefined;
  let mode: InstanceType<Sdk["InteractiveMode"]> | undefined;
  const restore = async () => {
    mode?.stop(); await host?.close(); assets.cleanup(); rmSync(work, { recursive: true, force: true });
    globalThis.fetch = oldFetch;
    if (oldState === undefined) delete process.env.PI_CODING_AGENT_DIR; else process.env.PI_CODING_AGENT_DIR = oldState;
  };
  try {
    restrictEnvironment(process.env); process.env.PI_CODING_AGENT_DIR = join(work, "pi-state");
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
    const original = await loadModules(assets.dir, "worker");
    const modules: Modules = { ...original, role: { ...original.role, extend(pi, context) { original.role.extend?.(pi, context); extend?.(pi, context); } } };
    const send = modules.auth.transport?.(authContext());
    if (!send) throw new Error("fixture transport unavailable");
    const events = new EventLog(1), network = guardedFetch(send);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const corral = writeFakeCorral(work);
    const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules, client, events, network,
      ...(options.capabilities ? { capabilities: options.capabilities } : {}),
      sessionManager: sdk.SessionManager.create(work, join(work, "sessions")) });
    await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
    const panels = host.panels;
    if (!panels) throw new Error("rich host did not install native panels");
    const physical = new RecordingTerminal(), terminal = panels.wrap(physical);
    let transcript: Component | undefined;
    let chatPresentation: PanelHost["chatPresentation"] | undefined;
    mode = new sdk.InteractiveMode(host.runtime, { terminal, tuiMode: "fullscreen", readOnlyQueue: true, verbose: false,
      mainView: (view, presentation) => { transcript = view; chatPresentation = presentation; return panels.mainView(view, presentation); } });
    await mode.init(); host.markAttached("interactive");
    void mode.run().catch(error => { throw error; });
    await physical.untilVisible("WORKER ● g1"); // the bottom bar's bound dot
    physical.send("\x1b[20~"); // task-bound sessions open on Task; F9 shows the transcript
    const view: Component | undefined = transcript;
    if (!(view instanceof ScrollView)) throw new Error("transcript is not a ScrollView");
    if (!chatPresentation) throw new Error("Native chat presentation was not supplied");
    const model = new ScreenModel(physical.columns, physical.rows);
    return {
      work, host, panels, physical, events, bound: host.sessionId(), transcript: view, chatPresentation,
      screen: () => model.sync(physical.writes),
      resize(columns, rows) { model.sync(physical.writes); physical.resize(columns, rows); model.resize(columns, rows); },
      stop: restore,
    };
  } catch (error) { await restore(); throw error; }
}
