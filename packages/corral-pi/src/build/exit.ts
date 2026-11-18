import { TOML } from "bun";
import { checked, json, type Run } from "./corral-cli.ts";
import { patchDigest, summarize } from "./git.ts";
import type { BuildJournal } from "./record.ts";
import type { GateReceipt, LandResult } from "./record-types.ts";
import { LIVE_CHILD_STATES, type BuildContext, type BuildSummary, type ExitChoice, type ExitControl, type ExitOption, type ExitRecord, type ExitState, type FleetSnapshot } from "./types.ts";

/** Recovery metadata stays local to session.json, not the tracked build record. */
export interface ExitProgressRecord extends ExitRecord {
  readonly progress?: { readonly phase: ExitState["phase"]; readonly detail?: string };
}
/** How many times one Land choice hands a failing full gate back to the session before it stops. */
export const LAND_REPAIR_ROUNDS = 3;

class GateFailed extends Error {
  constructor(readonly receipt: GateReceipt) {
    super(`Full gate failed${receipt.failed_command ? `: ${receipt.failed_command}` : ""}${receipt.log ? `; log: ${receipt.log}` : ""}`);
  }
}

function repairPrompt(receipt: GateReceipt, round: number, review: boolean): string {
  return [
    `[corral build] Land stopped: the full gate failed on the rebased head ${receipt.head}.`,
    ...(receipt.failed_command ? [`Failed command: ${receipt.failed_command}`] : []),
    ...(receipt.log ? [`Log: ${receipt.log}`] : []),
    `This is automatic repair ${round} of ${LAND_REPAIR_ROUNDS} for the owner's Land choice. It comes from the host, not the owner.`,
    "Read the log, fix the cause in this worktree and commit the fix. The branch is already rebased onto the base branch.",
    ...(review ? ["Then run build_review on the new head and resolve every finding: Land resumes only when an approving review covers it."] : []),
    "Do not call build_finish. When this turn ends with a new commit and a clean worktree, the host rebases, runs the full gate again and lands.",
    "If you cannot fix it, explain why and end the turn without committing; Land then stops for the owner.",
  ].join("\n");
}

export function createExit(options: {
  context: BuildContext; run: Run; git: Run; fleet: () => FleetSnapshot; journal: BuildJournal;
  save: (record: ExitRecord) => void; changed: () => void; now?: () => number;
  integrationBranch?: () => Promise<string | undefined>;
  /** Gives the parent a failed landing gate to fix; settles when the turn it starts ends. Absent: a failed gate ends Land. */
  repair?: (prompt: string) => Promise<{ readonly aborted: boolean }>;
}): ExitControl & { open(): void; close(): void } {
  const { context, run, git, journal } = options;
  const session = context.session;
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const saved = session.exit as ExitProgressRecord | undefined;
  let state: ExitState = saved ? { record: saved, phase: saved.progress?.phase ?? (saved.landed ? "landed" : saved.choice === "keep" ? "kept" : "none"), detail: saved.progress?.detail } : { phase: "none" };
  if (["rebasing", "verifying", "repairing", "landing"].includes(state.phase)) state = { ...state, phase: "failed", detail: `The previous exit stopped during ${state.phase}. Inspect the worktree and choose Land again when ready.` };
  let summary: BuildSummary | undefined;
  let integration: string | undefined;
  let integrationError: string | undefined;
  let inFlight: Promise<ExitState> | undefined;
  let closed = false;
  function changed() {
    for (const listener of [options.changed, ...listeners]) { try { listener(); } catch { /* Observers do not own exit execution. */ } }
  }
  function update(phase: ExitState["phase"], detail: string, record = state.record) {
    state = { phase, detail, ...(record ? { record } : {}) };
    if (record) options.save({ ...record, progress: { phase, detail } } as ExitProgressRecord);
    changed();
  }
  async function getSummary() {
    summary = session.bare && session.repository === false
      ? { branch: null, base: null, head: null, commits: 0, files: 0, added: 0, removed: 0, dirty: false, slices: [], outside: [] }
      : await summarize(git, session, options.fleet());
    integrationError = undefined;
    if (session.bare) {
      integration = session.baseBranch ?? undefined;
      if (session.repository !== false && !integration) integrationError = "The session started without a base branch.";
    } else if (session.workspaceRoot && session.project) {
      try {
        if (options.integrationBranch) integration = await options.integrationBranch();
        else {
          const projects = TOML.parse(await checked(run, ["--root", session.workspaceRoot, "config", "get", "projects"], session.workspaceRoot)) as Record<string, { base?: string }>;
          integration = projects[session.project]?.base ?? "main";
        }
        if (!integration) integrationError = "The project's base branch is not configured.";
      } catch (error) { integrationError = `Cannot read the project's base branch: ${error instanceof Error ? error.message : String(error)}`; }
    }
    changed(); return summary;
  }
  function choices(): readonly ExitOption[] {
    let unavailable: string | undefined;
    if (!session.bare && !session.workspaceRoot) unavailable = "This session is outside a corral workspace.";
    else if (!session.bare && !session.project) unavailable = "This session has no configured corral project.";
    else if (!session.bare && context.settings.exitPolicy === "none") unavailable = "Project policy disables landing.";
    else if (session.bare && session.repository === false) unavailable = "This session is not in a Git repository.";
    else if (!summary) unavailable = "The branch summary is still being read.";
    else if (integrationError) unavailable = integrationError;
    else if (!summary.branch) unavailable = "A detached session has no branch to land.";
    else if (session.here && summary.branch === integration) unavailable = session.bare ? "The session is on its starting branch; use a separate branch." : "The session is on the project's base branch; use a separate branch.";
    else if (!summary.base || summary.commits === 0) unavailable = "There are no commits beyond the session base.";
    else if (summary.dirty) unavailable = "Commit uncommitted changes before landing.";
    else if (options.fleet().children.some(child => LIVE_CHILD_STATES.includes(child.state))) unavailable = "Wait for all live children before landing.";
    const review = journal.latestReview();
    const needsReason = !session.bare && context.settings.exitPolicy === "review" && (!review || review.verdict !== "approve" || review.head !== summary?.head
      || review.findings.some(finding => !finding.resolution || finding.resolution.state === "open"))
      ? "No approving review covers the current head with every finding resolved. Give a reason to land anyway." : undefined;
    return [
      { choice: "keep", label: "Keep", detail: "Keep this worktree and branch and record the result." },
      ...(session.bare && session.repository === false ? [] : [
        { choice: "land" as const, label: "Land", detail: session.bare ? "Rebase and locally fast-forward the starting branch (no gate)." : "Rebase, run the full gate and fast-forward the project's base branch.", unavailable, ...(needsReason ? { needsReason } : {}) },
      ]),
      { choice: "iterate", label: "Iterate", detail: "Return to the session and continue working." },
    ];
  }
  async function primaryCheckout(): Promise<string> {
    const entries = await checked(git, ["worktree", "list", "--porcelain", "-z"], session.worktree);
    const first = entries.split("\0")[0];
    if (!first?.startsWith("worktree ")) throw new Error("Cannot locate the repository's primary checkout.");
    return first.slice("worktree ".length);
  }
  async function checkPrimary(primary: string): Promise<void> {
    const branch = await git(["symbolic-ref", "--quiet", "--short", "HEAD"], { cwd: primary });
    if (branch.code !== 0 || branch.stdout.trim() !== integration) {
      throw new Error(`The repository's primary checkout must be on ${integration}; found ${branch.stdout.trim() || "a detached HEAD"}.`);
    }
    if ((await checked(git, ["status", "--porcelain", "--untracked-files=no"], primary)).trim()) {
      throw new Error("The repository's primary checkout has tracked changes; commit or stash them before landing.");
    }
  }
  async function rebase(): Promise<void> {
    const result = await git(["rebase", integration!], { cwd: session.worktree });
    if (result.code === 0) return;
    const conflicts = await git(["diff", "--name-only", "--diff-filter=U"], { cwd: session.worktree });
    const abort = await git(["rebase", "--abort"], { cwd: session.worktree });
    throw new Error(`Rebase failed${conflicts.stdout.trim() ? `; conflicted paths: ${conflicts.stdout.trim().split("\n").join(", ")}` : `: ${result.stderr.trim() || result.stdout.trim()}`}.${abort.code ? ` Rebase abort failed: ${abort.stderr.trim()}` : " The rebase was aborted; the branch is unchanged."}`);
  }
  /** One Land attempt: rebase, full gate, fast-forward. Throws `GateFailed` when the gate fails. */
  async function land(record: ExitRecord, warnings: string[]): Promise<void> {
    for (let attempt = 0; attempt < 3; attempt++) {
      update("rebasing", `Rebasing onto ${integration}.`);
      journal.timeline("land", `Rebasing onto ${integration}.`);
      const tip = (await checked(git, ["rev-parse", integration!], session.worktree)).trim();
      const patchBase = (await checked(git, ["merge-base", tip, "HEAD"], session.worktree)).trim();
      const before = await patchDigest(git, patchBase, session.worktree);
      await rebase();
      const after = await patchDigest(git, tip, session.worktree);
      if (context.settings.exitPolicy === "review" && before !== after && !record.overrideReason) throw new Error("The rebase changed the patch. Review the rebased head again with build_review, then choose Land.");
      const head = (await checked(git, ["rev-parse", "HEAD"], session.worktree)).trim();
      update("verifying", "Running the full gate on the rebased head.");
      journal.timeline("verify", `Full gate on ${head}.`);
      const receipt = await json<GateReceipt>(run, ["build", "gate", "--session", session.id, "--worktree", session.worktree,
        "--project", session.project!, "--tier", "full", "--base", session.base!, "--json"], session.workspaceRoot!, 2 * 60 * 60 * 1000);
      journal.gate(receipt);
      try { await journal.refresh(); } catch (error) { warnings.push(`Build record Git snapshot failed: ${String(error)}`); }
      const publication = await journal.publish();
      if (publication) warnings.push(publication);
      if (receipt.result === "fail") throw new GateFailed(receipt);
      update("landing", `Landing ${head} with gate ${receipt.attempt}.`);
      journal.timeline("land", `Landing ${head} with gate ${receipt.attempt}.`);
      const landed = await run(["build", "land", "--session", session.id, "--worktree", session.worktree, "--project", session.project!,
        "--expected-head", head, "--gate-attempt", receipt.attempt, "--json"], { cwd: session.workspaceRoot! });
      if (landed.code !== 0) {
        const refusal = landed.stderr.trim() || landed.stdout.trim();
        if (refusal.startsWith("landing_stale:") && attempt < 2) continue;
        throw new Error(refusal || `Landing exited ${landed.code}`);
      }
      let result: LandResult;
      try { result = JSON.parse(landed.stdout) as LandResult; } catch { throw new Error("Landing returned invalid JSON; inspect the base branch before retrying."); }
      const final = { ...record, landed: result.landed, baseBranch: result.base_branch, gateAttempt: receipt.attempt };
      journal.outcome({ state: "landed", at: new Date(now()).toISOString(), head, landed: result.landed, base_branch: result.base_branch, override_reason: record.overrideReason });
      journal.timeline("land", `${result.base_branch}: ${result.landed}`);
      update("landed", `Landed at ${result.landed} on ${result.base_branch}.`, final);
      return;
    }
  }
  /** Hands a failed landing gate to the session; returns once Land may run again and throws when it may not. */
  async function repaired(failure: GateFailed, round: number, record: ExitRecord): Promise<void> {
    const stopped = (why: string) => new Error(`${failure.message}. Land did not resume after repair ${round}: ${why}`);
    update("repairing", `${failure.message}. Repair ${round} of ${LAND_REPAIR_ROUNDS}: the session is fixing it and Land resumes when it finishes.`);
    journal.timeline("land", `Repair ${round} of ${LAND_REPAIR_ROUNDS}: ${failure.message}`);
    let turn: { readonly aborted: boolean };
    try { turn = await options.repair!(repairPrompt(failure.receipt, round, context.settings.exitPolicy === "review" && !record.overrideReason)); }
    catch (error) { throw stopped(`the session could not be reached (${error instanceof Error ? error.message : String(error)}).`); }
    if (closed) throw stopped("the session closed.");
    if (turn.aborted) throw stopped("the repair turn was stopped.");
    await getSummary();
    const option = choices().find(option => option.choice === "land");
    if (option?.unavailable) throw stopped(option.unavailable);
    if (summary?.head === failure.receipt.head) throw stopped("the session made no new commit.");
    if (option?.needsReason && !record.overrideReason) throw stopped("no approving review covers the repaired head with every finding resolved.");
  }
  async function execute(choice: ExitChoice, overrideReason?: string): Promise<void> {
    const warnings: string[] = [];
    journal.decision("exit", choice);
    journal.timeline("exit", choice);
    try {
      if (choice === "iterate") { update("none", "The build session remains open for iteration."); return; }
      if (choice === "keep") {
        try { summary = session.bare && session.repository === false ? await getSummary() : await summarize(git, session, options.fleet()); } catch { /* Keeping an unborn or unavailable worktree does not require a Git head. */ }
      } else await getSummary();
      const option = choices().find(option => option.choice === choice);
      if (!option) throw new Error("Land is unavailable outside a Git repository.");
      if (option.unavailable) throw new Error(option.unavailable);
      if (choice === "land" && option.needsReason && !overrideReason?.trim()) throw new Error(option.needsReason);
      const record: ExitRecord = { choice, at: new Date(now()).toISOString(), ...(overrideReason?.trim() ? { overrideReason: overrideReason.trim() } : {}) };
      state = { ...state, record };
      if (choice === "keep") {
        journal.outcome({ state: "kept", at: record.at, head: summary?.head ?? undefined });
        journal.timeline("exit", "kept");
        update("kept", "The worktree and branch are kept.", record);
        return;
      }
      if (session.bare) {
        const primary = await primaryCheckout();
        await checkPrimary(primary);
        update("rebasing", `Rebasing onto ${integration}.`);
        journal.timeline("land", `Rebasing onto ${integration}.`);
        await rebase();
        const head = (await checked(git, ["rev-parse", "HEAD"], session.worktree)).trim();
        await checkPrimary(primary);
        update("landing", `Locally fast-forwarding ${integration} to ${head}.`);
        await checked(git, ["merge", "--ff-only", head], primary);
        journal.outcome({ state: "landed", at: new Date(now()).toISOString(), head, landed: head, base_branch: integration });
        journal.timeline("land", `${integration}: ${head}`);
        update("landed", `Landed at ${head} on ${integration}.`, { ...record, landed: head, baseBranch: integration });
        return;
      }
      if (record.overrideReason) journal.decision("override", record.overrideReason);
      for (let round = 1; ; round++) {
        try { await land(record, warnings); return; }
        catch (error) {
          if (!(error instanceof GateFailed) || !options.repair || round > LAND_REPAIR_ROUNDS) throw error;
          await repaired(error, round, record);
        }
      }
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      journal.timeline("exit", `failed: ${detail}`);
      update("failed", detail);
    } finally {
      try { await journal.refresh(); } catch (error) { warnings.push(`Build record Git snapshot failed: ${error instanceof Error ? error.message : String(error)}`); }
      const publication = await journal.publish();
      if (publication) warnings.push(publication);
      if (warnings.length) update(state.phase, `${state.detail ?? ""}\n${[...new Set(warnings)].join("\n")}`);
    }
  }
  return {
    options: choices, summary: getSummary, state: () => state,
    choose(choice, options) {
      if (closed) return Promise.resolve(state);
      if (state.phase === "landed" && choice !== "iterate") return Promise.resolve(state);
      if (inFlight) return inFlight;
      inFlight = execute(choice, options?.overrideReason).then(() => state).finally(() => { inFlight = undefined; });
      return inFlight;
    },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    open() { if (closed || inFlight) return; update("pending", "Choose Keep, Land or Iterate."); void getSummary().catch(error => update("failed", String(error))); },
    close() { closed = true; listeners.clear(); },
  };
}
