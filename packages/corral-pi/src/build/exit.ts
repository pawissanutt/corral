import { TOML } from "bun";
import { checked, json, type Run, type TaskReadback } from "./corral-cli.ts";
import { summarize } from "./git.ts";
import type { BuildContext, BuildSummary, ExitChoice, ExitControl, ExitOption, ExitRecord, ExitState, FleetSnapshot } from "./types.ts";

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
  let state: ExitState = session.exit ? {
    phase: session.exit.choice === "keep" ? "kept" : session.exit.choice === "land" ? "landing" : "shipped", record: session.exit,
  } : { phase: "none" };
  let summary: BuildSummary | undefined;
  let integration: string | undefined;
  let integrationRead = !session.here;
  let integrationError: string | undefined;
  let inFlight: Promise<ExitState> | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let closed = false;
  let assistant: string | undefined = session.exit?.assistant;
  let scratch: string | undefined;
  let task: string | undefined = session.exit?.task;
  function changed() { options.changed(); for (const listener of listeners) listener(); }
  function update(phase: ExitState["phase"], detail: string) {
    state = { ...state, phase, detail }; changed();
  }
  function retained() {
    const parts = [assistant && `session ${assistant}`, scratch && `scratch ${scratch}`, task && `task ${task}`].filter(Boolean);
    return parts.length ? ` Existing ${parts.join(", ")} remain available for manual continuation.` : "";
  }
  function failed(error: unknown) {
    update("failed", `${error instanceof Error ? error.message : String(error)}${retained()}`);
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
    else if (session.here && session.branch === null) reason = "A detached session has no branch to ship.";
    else if (session.here && integrationError) reason = integrationError;
    else if (session.here && !integrationRead) reason = "The project's integration branch is still being read.";
    else if (session.here && (!integration || integration === session.branch)) reason = "The session wrote on the project's base branch; there is no separate branch to ship.";
    else if (!summary) reason = "The branch summary is still being read.";
    else if (!summary.base || summary.commits === 0) reason = "There are no commits beyond the session base.";
    else if (summary.dirty) reason = "Commit or remove uncommitted changes before shipping.";
    return [
      { choice: "keep", label: "Keep on the branch", detail: "Keep this worktree and its branch without creating a corral task." },
      { choice: "land", label: "Land after the gate", detail: "Submit the branch and waive independent review after the required gate.",
        unavailable: root && context.settings.exitPolicy === "review" ? "Project policy requires independent review." : reason },
      { choice: "ship", label: "Ship through the board", detail: "Submit the branch for the gate and independent review.", unavailable: reason },
      { choice: "iterate", label: "Keep iterating", detail: "Return to the build session without choosing an exit." },
    ];
  }
  function record(choice: ExitChoice, reviewWaived = false) {
    const value: ExitRecord = { choice, at: new Date(now()).toISOString(), ...(task ? { task } : {}),
      ...(assistant ? { assistant } : {}), ...(reviewWaived ? { reviewWaived: true } : {}) };
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
      if (choice === "keep") { record(choice); update("kept", "The worktree and branch are kept without a corral task."); return state; }
      update("running", "Checking the committed branch before submitting it to corral.");
      const value = await getSummary();
      const unavailable = choices().find(option => option.choice === choice)?.unavailable;
      if (unavailable) throw new Error(unavailable);
      if (!root || !session.project || !value.base || !value.head) throw new Error("The session has no complete corral submission context.");
      const slug = `build-${session.id.toLowerCase().replace(/[^a-z0-9-]/g, "-")}`.slice(0, 40);
      update("running", "Creating the assistant session that will carry this branch.");
      const created = await json<{ id: string }>(run, ["assistant", "create", "--profile", "pi", "--json"], root);
      if (!created.id) throw new Error("corral assistant create returned no session id.");
      assistant = created.id;
      update("running", `Creating a scratch for session ${assistant}.`);
      const createdScratch = await json<{ path: string; branch: string; base: string }>(run,
        ["assistant", "scratch", "new", "--project", session.project, "--slug", slug, "--base", value.base, "--session", assistant, "--json"], root);
      if (!createdScratch.path) throw new Error("corral scratch new returned no scratch path.");
      scratch = createdScratch.path;
      update("running", `Fast-forwarding scratch ${scratch} to the build candidate.`);
      await checked(git, ["-C", scratch, "merge", "--ff-only", value.head], root);
      update("running", `Submitting scratch ${scratch} from session ${assistant}.`);
      const finished = await json<{ task: string }>(run,
        ["assistant", "scratch", "finish", slug, "--title", session.name, "--intent", "ship", "--expected-head", value.head, "--session", assistant, "--json"], root);
      if (!finished.task) throw new Error("corral scratch finish returned no task id.");
      task = finished.task;
      record(choice);
      if (choice === "ship") { update("shipped", `${task} was submitted through session ${assistant} for the gate and independent review.`); return state; }
      update("running", `Waiving independent review for ${task} while retaining the required gate.`);
      await checked(run, ["waive-review", task, "--reason", "build session exit: project policy requires the gate only"], root);
      record(choice, true);
      update("running", `Synchronizing corral so ${task} can advance through its gate.`);
      await checked(run, ["sync"], root);
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
