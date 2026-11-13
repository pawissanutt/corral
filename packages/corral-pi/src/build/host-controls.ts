import { dirname, join } from "node:path";
import type { ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { FleetRuntime, type FleetOptions } from "../extensions/fleet/runtime.ts";
import { fleetTools } from "../extensions/fleet/tools.ts";
import { createSdkRunner, type ChildRunner } from "../extensions/fleet/child.ts";
import { createBoard } from "./board.ts";
import { gitRun, ownerRun, type Run } from "./corral-cli.ts";
import { createExit } from "./exit.ts";
import { loadSession, saveSession } from "./session-store.ts";
import { boardPlanTool, buildFinishTool } from "./tools.ts";
import { LIVE_CHILD_STATES, type BoardControl, type BoardPlan, type BuildPanelState, type BuildUi, type ExitControl } from "./types.ts";

export interface BuildControlsOptions extends Omit<FleetOptions, "models" | "approved" | "localTools"> {
  readonly models: () => ModelRuntime;
  readonly approved: () => readonly string[];
  readonly localTools: () => readonly ToolDefinition[];
  readonly run?: Run;
  readonly git?: Run;
}

export interface BuildControls {
  readonly fleet: FleetRuntime;
  readonly exit: ExitControl & { open(): void; close(): void };
  readonly board: BoardControl & { propose(plan: BoardPlan): void; close(): void };
  readonly tools: ToolDefinition[];
  readonly ui: BuildUi;
  panelState(): BuildPanelState;
  close(): Promise<void>;
}

export function createBuildControls(options: BuildControlsOptions): BuildControls {
  const { context } = options;
  const dataRoot = dirname(dirname(context.dir));
  let session = context.session;
  let target = "parent";
  let sdkRunner: ChildRunner | undefined;
  const runner: ChildRunner = options.runner ?? { run(...args) {
    sdkRunner ??= createSdkRunner({ ...options, models: options.models(), localTools: options.localTools() });
    return sdkRunner.run(...args);
  } };
  const fleet: FleetRuntime = new FleetRuntime({
    ...options, runner,
    get models() { return options.models(); },
    get approved() { return options.approved(); },
    get localTools() { return options.localTools(); },
    changed: () => {
      if (target !== "parent" && !fleet.snapshot().children.some(child => child.id === target && LIVE_CHILD_STATES.includes(child.state))) target = "parent";
      options.changed();
    },
  });
  const run = options.run ?? ownerRun(context.corral);
  const exit = createExit({ context, run, git: options.git ?? gitRun(), fleet: () => fleet.snapshot(),
    changed: options.changed, now: options.now,
    save(record) {
      // The entry records the transcript after binding, later than this composition is created.
      const saved = loadSession(dataRoot, session.id);
      const updated = { ...saved, exit: record, updated: new Date().toISOString() };
      saveSession(dataRoot, updated);
      session = updated;
    },
  });
  const board = createBoard({ context, run, tmpDir: join(context.dir, "board"), changed: options.changed, now: options.now });
  const tools = [...fleetTools(fleet), buildFinishTool(() => exit.open()), boardPlanTool(plan => board.propose(plan), () => {
    if (!session.workspaceRoot) return "This session is outside a corral workspace.";
    if (!session.project) return "This session has no configured corral project.";
    return undefined;
  })];
  const ui: BuildUi = {
    fleet, exit, board, target: () => target,
    setTarget(id) {
      const next = id === "parent" || fleet.snapshot().children.some(child => child.id === id && LIVE_CHILD_STATES.includes(child.state)) ? id : "parent";
      if (target === next) return;
      target = next;
      options.changed();
    },
  };
  return {
    fleet, exit, board, tools, ui,
    panelState: () => ({ session, settings: context.settings, fleet: fleet.snapshot(), target, exit: exit.state(), board: board.tasks() }),
    async close() {
      exit.close();
      board.close();
      await fleet.close();
    },
  };
}
