import { dirname, join } from "node:path";
import type { ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { FleetRuntime, type FleetOptions } from "../extensions/fleet/runtime.ts";
import { fleetTools } from "../extensions/fleet/tools.ts";
import type { InputWake } from "../extensions/input-wake.ts";
import { createSdkRunner, type ChildRunner } from "../extensions/fleet/child.ts";
import { createBoard } from "./board.ts";
import { engineRun, gitRun, type Run } from "./corral-cli.ts";
import { createExit } from "./exit.ts";
import { loadSession, saveSession } from "./session-store.ts";
import { boardPlanTool, buildEvidenceTools, buildFinishTool } from "./tools.ts";
import { createJournal, type BuildJournal } from "./record.ts";
import { LIVE_CHILD_STATES, type BoardControl, type BoardPlan, type BuildContext, type BuildPanelState, type BuildUi, type ExitControl, type FleetControl } from "./types.ts";
import { buildShellRefusal } from "../roles/build/guard.ts";

export interface BuildControlsOptions extends Omit<FleetOptions, "models" | "approved" | "localTools" | "context"> {
  readonly context: BuildContext;
  readonly models: () => ModelRuntime;
  readonly apiModels?: Parameters<typeof createSdkRunner>[0]["apiModels"];
  readonly approved: () => readonly string[];
  readonly localTools: () => readonly ToolDefinition[];
  readonly run?: Run;
  readonly git?: Run;
  /** Ends the parent's agent_wait when input is queued for its next turn boundary. */
  readonly wake?: InputWake;
}

export interface BuildControls {
  readonly fleet: FleetRuntime;
  readonly exit: ExitControl & { open(): void; close(): void };
  readonly board?: BoardControl & { propose(plan: BoardPlan): void; close(): void };
  readonly tools: ToolDefinition[];
  readonly ui: BuildUi;
  panelState(): BuildPanelState;
  noteOwnerInput(text: string): void;
  close(): Promise<void>;
}

export function createBuildControls(options: BuildControlsOptions): BuildControls {
  const { context } = options;
  const dataRoot = dirname(dirname(context.dir));
  let session = context.session;
  let target = "parent";
  let sdkRunner: ChildRunner | undefined;
  let journal: BuildJournal | undefined;
  const runner: ChildRunner = options.runner ?? { run(...args) {
    sdkRunner ??= createSdkRunner({ ...options, models: options.models(), localTools: options.localTools(),
      ...(context.session.bare ? { shellRefusal: (command: string) => buildShellRefusal(command, true) } : {}) });
    return sdkRunner.run(...args);
  } };
  const fleet: FleetRuntime = new FleetRuntime({
    ...options, runner, deliverToParent: options.deliverToParent,
    get models() { return options.models(); },
    get approved() { return options.approved(); },
    get localTools() { return options.localTools(); },
    changed: () => {
      if (target !== "parent" && !fleet.snapshot().children.some(child => child.id === target && LIVE_CHILD_STATES.includes(child.state))) target = "parent";
      journal?.capture();
      options.changed();
    },
  });
  const run = options.run ?? engineRun(context.corral);
  const git = options.git ?? gitRun();
  journal = createJournal({ context, git, fleet: () => fleet.snapshot(), changed: options.changed, now: options.now,
    session: () => loadSession(dataRoot, session.id) });
  const buildJournal = journal;
  const initialized = buildJournal.refresh().catch(error => {
    try { void Promise.resolve(options.deliverToParent(`Build record Git snapshot failed: ${error instanceof Error ? error.message : String(error)}`)).catch(() => {}); }
    catch { /* A reporting failure does not block the interactive session. */ }
  });
  const exit = createExit({ context, git, fleet: () => fleet.snapshot(), journal: buildJournal,
    changed: options.changed, now: options.now,
    save(record) {
      // The entry records the transcript after binding, later than this composition is created.
      const saved = loadSession(dataRoot, session.id);
      const updated = { ...saved, exit: record, updated: new Date().toISOString() };
      saveSession(dataRoot, updated);
      session = updated;
    },
  });
  const board = context.session.bare ? undefined : createBoard({ context, run, tmpDir: join(context.dir, "board"), changed: options.changed, now: options.now });
  const fleetDefinitions = fleetTools(fleet, undefined, options.wake);
  const task = fleetDefinitions.find(tool => tool.name === "task")!;
  const start = task.execute;
  const previousIds = new Set(buildJournal.record().slices.map(child => child.id));
  task.execute = async (...args) => {
    const params = args[1] as { agents: { id: string }[] };
    const reused = params.agents.find(child => previousIds.has(child.id));
    if (reused) return { content: [{ type: "text", text: `Child id ${reused.id} belongs to this session's saved history. Choose a new id.` }], details: {}, isError: true };
    return start(...args);
  };
  const tools = [...fleetDefinitions, ...buildEvidenceTools({ context, git, fleet, journal: buildJournal }), buildFinishTool(() => exit.open(), buildJournal),
    ...(board ? [boardPlanTool(plan => board.propose(plan), () => {
      if (!session.workspaceRoot) return "This session is outside a corral workspace.";
      if (!session.project) return "This session has no configured corral project.";
      return undefined;
    })] : [])];
  const uiFleet: FleetControl = {
    snapshot: () => fleet.snapshot(), steer: (id, text) => fleet.steer(id, text), cancel: id => fleet.cancel(id),
    setModel: (id, model) => fleet.setModel(id, model), models: () => fleet.models(), profiles: () => fleet.profiles(),
    message: (from, to, text, options) => fleet.message(from, to, text, options), roster: () => fleet.roster(),
    wait: (ids, signal) => fleet.wait(ids, signal), subscribe: listener => fleet.subscribe(listener),
    answer(id, text) {
      const question = fleet.snapshot().children.find(child => child.id === id)?.question;
      const answered = fleet.answer(id, text);
      if (answered && question) buildJournal.decision("question", question.question, text);
      return answered;
    },
  };
  const ui: BuildUi = {
    fleet: uiFleet, exit, board, record: () => buildJournal.record(), target: () => target,
    setTarget(id) {
      const next = id === "parent" || fleet.snapshot().children.some(child => child.id === id && LIVE_CHILD_STATES.includes(child.state)) ? id : "parent";
      if (target === next) return;
      target = next;
      options.changed();
    },
  };
  return {
    fleet, exit, board, tools, ui,
    panelState: () => ({ session, settings: context.settings, fleet: fleet.snapshot(), target, exit: exit.state(), record: buildJournal.record(), board: board?.tasks() ?? [] }),
    noteOwnerInput(text) {
      try { buildJournal.noteOwnerInput(text); }
      catch (error) {
        try { void Promise.resolve(options.deliverToParent(`Build journal could not record owner input: ${error instanceof Error ? error.message : String(error)}`)).catch(() => {}); }
        catch { /* Owner input must still reach the model when reporting is unavailable. */ }
      }
    },
    async close() {
      exit.close();
      board?.close();
      await fleet.close();
      await initialized;
      await buildJournal.refresh().catch(error => {
        return Promise.resolve(options.deliverToParent(`Build record Git snapshot failed: ${String(error)}`)).catch(() => {});
      });
    },
  };
}
