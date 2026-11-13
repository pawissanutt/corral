import { TOML } from "bun";
import { checked, json, type Run, type TaskReadback } from "./corral-cli.ts";
import { summarize } from "./git.ts";
import type { BuildContext, BuildSummary, ExitChoice, ExitControl, ExitOption, ExitRecord, ExitState, FleetSnapshot } from "./types.ts";

export type ExitStep = "none" | "assistant" | "scratch" | "merged" | "submitted" | "waived" | "synced";

/** Optional journal data travels with ExitRecord through the session store's JSON round-trip. */
export interface ExitProgressRecord extends ExitRecord {
  readonly progress?: {
    readonly step: ExitStep;
    readonly phase: ExitState["phase"];
    readonly detail?: string;
    readonly scratch?: string;
    readonly head?: string;
  };
}

const NEXT_STEP: Record<ExitStep, string> = {
  none: "assistant session creation", assistant: "scratch creation", scratch: "Git fast-forward",
  merged: "scratch submission", submitted: "waive-review", waived: "sync", synced: "landing",
};

export function createExit(options: {
  context: BuildContext; run: Run; git: Run; fleet: () => FleetSnapshot;
  save: (record: ExitRecord) => void; changed: () => void; now?: () => number; pollMs?: number;
  integrationBranch?: () => Promise<string | undefined>;
}): ExitControl & { open(): void; close(): void } {
  const { context, run, git } = options;
  const session = context.session;
  const root = session.workspaceRoot;
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const saved = session.exit as ExitProgressRecord | undefined;
  let assistant = saved?.assistant;
  let scratch = saved?.progress?.scratch;
  let task = saved?.task;
  let candidate = saved?.progress?.head;
  let step: ExitStep = saved?.progress?.step ?? (task ? (saved?.reviewWaived ? "waived" : "submitted") : assistant ? "assistant" : "none");
  let activeChoice = saved?.choice;
  let exitAt = saved?.at;
  let state: ExitState = { phase: "none" };
  if (saved) {
    if (saved.progress) {
      const interrupted = saved.progress.phase === "running"
        || (saved.choice === "land" && Boolean(task) && step !== "synced" && saved.progress.phase !== "failed");
      state = { phase: interrupted ? "failed" : saved.progress.phase, record: saved,
        detail: interrupted ? `The exit was interrupted before ${NEXT_STEP[step]} completed; choose again to continue.${retained()}` : saved.progress.detail };
    } else if (saved.choice === "land") {
      state = { phase: "failed", record: saved,
        detail: `The saved land exit has no confirmed ${NEXT_STEP[step]} completion; choose again to continue.${retained()}` };
    } else {
      state = { phase: saved.choice === "keep" ? "kept" : saved.choice === "ship" ? "shipped" : "none", record: saved };
    }
  }
  let summary: BuildSummary | undefined;
  let integration: string | undefined;
  let integrationRead = !session.here;
  let integrationError: string | undefined;
  let inFlight: Promise<ExitState> | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let closed = false;
  function changed() { options.changed(); for (const listener of listeners) listener(); }
  function update(phase: ExitState["phase"], detail: string) {
    state = { ...state, phase, detail }; persist(); changed();
  }
  function retained() {
    const parts = [assistant && `session ${assistant}`, scratch && `scratch ${scratch}`, task && `task ${task}`].filter(Boolean);
    return parts.length ? ` Existing ${parts.join(", ")} remain available for manual continuation.` : "";
  }
  function failed(error: unknown) {
    update("failed", `${NEXT_STEP[step]} failed: ${error instanceof Error ? error.message : String(error)}${retained()}`);
  }
  async function readIntegration() {
    if (!session.here || !root || !session.project) return;
    try {
      integration = options.integrationBranch ? await options.integrationBranch() : await (async () => {
        const text = await checked(run, ["--root", root, "config", "get", "projects"], root);
        const projects = TOML.parse(text) as Record<string, { base?: string }>;
        return projects[session.project!]?.base;
      })();
      integrationRead = true;
      integrationError = undefined;
    } catch (error) {
      integrationError = `Cannot read the project's integration branch: ${error instanceof Error ? error.message : String(error)}`;
      integrationRead = true;
    }
  }
  async function getSummary() {
    const [value] = await Promise.all([summarize(git, session, options.fleet()), readIntegration()]);
    summary = value; changed(); return value;
  }
  function choices(): readonly ExitOption[] {
    let reason: string | undefined;
    if (!root) reason = "This session is outside a corral workspace.";
    else if (!session.project) reason = "This session has no configured corral project.";
    else if (context.settings.exitPolicy === "none") reason = "Project policy disables exits through corral.";
    else if (!task) {
      if (!summary) reason = "The branch summary is still being read.";
      else if (session.here && summary.branch === null) reason = "A detached session has no branch to ship.";
      else if (session.here && integrationError) reason = integrationError;
      else if (session.here && !integrationRead) reason = "The project's integration branch is still being read.";
      else if (session.here && (!integration || integration === summary.branch)) reason = "The session wrote on the project's base branch; there is no separate branch to ship.";
      else if (!summary.base || summary.commits === 0) reason = "There are no commits beyond the session base.";
      else if (summary.dirty) reason = "Commit or remove uncommitted changes before shipping.";
    }
    return [
      { choice: "keep", label: "Keep on the branch", detail: "Keep this worktree and its branch without creating a corral task." },
      { choice: "land", label: "Land after the gate", detail: "Submit the branch and waive independent review after the required gate.",
        unavailable: root && context.settings.exitPolicy === "review" ? "Project policy requires independent review." : reason },
      { choice: "ship", label: "Ship through the board", detail: "Submit the branch for the gate and independent review.", unavailable: reason },
      { choice: "iterate", label: "Keep iterating", detail: "Return to the build session without choosing an exit." },
    ];
  }
  function persist() {
    if (!activeChoice) return;
    exitAt ??= new Date(now()).toISOString();
    const value: ExitProgressRecord = { choice: activeChoice, at: exitAt, ...(task ? { task } : {}),
      ...(assistant ? { assistant } : {}), ...(["waived", "synced"].includes(step) ? { reviewWaived: true } : {}),
      progress: { step, phase: state.phase, detail: state.detail, ...(scratch ? { scratch } : {}), ...(candidate ? { head: candidate } : {}) } };
    options.save(value);
    state = { ...state, record: value };
  }
  async function poll() {
    if (closed || state.phase !== "landing" || !task || !root) return;
    try {
      update("landing", `Checking whether ${task} has landed.`);
      const value = await json<TaskReadback>(run, ["show", task, "--json"], root);
      if (closed) return;
      const current = value.runs?.at(-1);
      if (value.landed) { update("landed", `${task} landed at ${value.landed.sha}.`); return; }
      if (current?.gate?.result === "fail") {
        failed(new Error(`The gate failed for ${task}${current.gate.failed_cmd ? `: ${current.gate.failed_cmd}` : "."}`)); return;
      }
      if (value.status.toLowerCase() !== "review") {
        failed(new Error(`${task} left Review without landing (${value.status})${current?.autopilot?.reason ? `: ${current.autopilot.reason}` : "."}`)); return;
      }
      update("landing", `Waiting for ${task} to pass its gate and land.`);
      timer = setTimeout(() => { void poll(); }, options.pollMs ?? 5000);
    } catch (error) { if (!closed) failed(error); }
  }
  async function execute(choice: ExitChoice): Promise<ExitState> {
    try {
      if (choice === "iterate") { update("none", "The build session remains open for iteration."); return state; }
      if (activeChoice !== choice) exitAt = undefined;
      activeChoice = choice;
      if (choice === "keep") { update("kept", "The worktree and branch are kept without a corral task."); return state; }
      let value: BuildSummary | undefined;
      if (!task) {
        update("running", "Checking the committed branch before submitting it to corral.");
        value = await getSummary();
      }
      const unavailable = choices().find(option => option.choice === choice)?.unavailable;
      if (unavailable) throw new Error(unavailable);
      if (!root || !session.project) throw new Error("The session has no complete corral submission context.");
      if (!task) {
        if (!value?.base || !value.head) throw new Error("The session has no complete Git submission context.");
        candidate ??= value.head;
        const slug = `build-${session.id.toLowerCase().replace(/[^a-z0-9-]/g, "-")}`.slice(0, 40);
        if (!assistant) {
          update("running", "Creating the assistant session that will carry this branch.");
          const created = await json<{ id: string }>(run, ["assistant", "create", "--profile", "pi", "--json"], root);
          if (!created.id) throw new Error("corral assistant create returned no session id.");
          assistant = created.id;
          step = "assistant";
        }
        if (!scratch) {
          update("running", `Creating a scratch for session ${assistant}.`);
          const createdScratch = await json<{ path: string; branch: string; base: string }>(run,
            ["assistant", "scratch", "new", "--project", session.project, "--slug", slug, "--base", value.base, "--session", assistant, "--json"], root);
          if (!createdScratch.path) throw new Error("corral scratch new returned no scratch path.");
          scratch = createdScratch.path;
          step = "scratch";
        }
        if (step !== "merged") {
          update("running", `Fast-forwarding scratch ${scratch} to the build candidate.`);
          await checked(git, ["-C", scratch, "merge", "--ff-only", candidate], root);
          step = "merged";
        }
        update("running", `Submitting scratch ${scratch} from session ${assistant}.`);
        const finished = await json<{ task: string }>(run,
          ["assistant", "scratch", "finish", slug, "--title", session.name, "--intent", "ship", "--expected-head", candidate, "--session", assistant, "--json"], root);
        if (!finished.task) throw new Error("corral scratch finish returned no task id.");
        task = finished.task;
        step = "submitted";
      }
      if (choice === "ship") { update("shipped", `${task} was submitted through session ${assistant} for the gate and independent review.`); return state; }
      if (step !== "waived" && step !== "synced") {
        update("running", `Waiving independent review for ${task} while retaining the required gate.`);
        await checked(run, ["waive-review", task, "--reason", "build session exit: project policy requires the gate only"], root);
        step = "waived";
      }
      if (step !== "synced") {
        update("running", `Synchronizing corral so ${task} can advance through its gate.`);
        await checked(run, ["sync"], root);
        step = "synced";
      }
      update("landing", `Waiting for ${task} to pass its gate and land.`);
      void poll();
    } catch (error) { failed(error); }
    return state;
  }
  if (state.phase === "landing") timer = setTimeout(() => { void poll(); }, options.pollMs ?? 5000);
  return {
    options: choices, summary: getSummary, state: () => state,
    open() {
      if (inFlight || ["kept", "shipped", "landing", "landed"].includes(state.phase)) return;
      update("pending", "Choose what should happen to this build branch.");
      void getSummary().catch(failed);
    },
    choose(choice) {
      if (inFlight) return inFlight;
      if (["kept", "shipped", "landing", "landed"].includes(state.phase)) return Promise.resolve(state);
      inFlight = Promise.resolve().then(() => execute(choice)).finally(() => { inFlight = undefined; });
      return inFlight;
    },
    close() { closed = true; clearTimeout(timer); },
    subscribe(listener) { listeners.add(listener); return () => { listeners.delete(listener); }; },
  };
}
