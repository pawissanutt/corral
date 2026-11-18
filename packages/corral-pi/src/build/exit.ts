import { checked, type Run } from "./corral-cli.ts";
import { buildProject, verifyBuild } from "./gate.ts";
import { patchDigest, summarize } from "./git.ts";
import type { BuildJournal } from "./record.ts";
import { LIVE_CHILD_STATES, type BuildContext, type BuildSummary, type ExitChoice, type ExitControl, type ExitOption, type ExitRecord, type ExitState, type FleetSnapshot } from "./types.ts";

/** Recovery metadata stays local to session.json, not the tracked build record. */
export interface ExitProgressRecord extends ExitRecord {
  readonly progress?: { readonly phase: ExitState["phase"]; readonly detail?: string };
}
export function createExit(options: {
  context: BuildContext; git: Run; fleet: () => FleetSnapshot; journal: BuildJournal;
  save: (record: ExitRecord) => void; changed: () => void; now?: () => number;
}): ExitControl & { open(): void; close(): void } {
  const { context, git, journal } = options;
  const session = context.session;
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const saved = session.exit as ExitProgressRecord | undefined;
  let state: ExitState = saved ? { record: saved, phase: saved.progress?.phase ?? (saved.landed ? "landed" : saved.choice === "keep" ? "kept" : "none"), detail: saved.progress?.detail } : { phase: "none" };
  if (["rebasing", "verifying", "landing"].includes(state.phase)) state = { ...state, phase: "failed", detail: `The previous exit stopped during ${state.phase}. Inspect the worktree and choose Land again when ready.` };
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
        integration = buildProject(context).base;
        if (!integration) {
          const remote = await git(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], { cwd: session.repoRoot });
          integration = remote.code === 0 ? remote.stdout.trim().replace(/^origin\//, "") : undefined;
        }
        if (!integration) {
          const current = await git(["symbolic-ref", "--quiet", "--short", "HEAD"], { cwd: session.repoRoot });
          if (current.code === 0) integration = current.stdout.trim();
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
        { choice: "land" as const, label: "Land", detail: session.bare ? "Rebase and locally fast-forward the starting branch (no gate)." : "Rebase, verify the project's gate and locally fast-forward its base branch.", unavailable, ...(needsReason ? { needsReason } : {}) },
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
  async function execute(choice: ExitChoice, overrideReason?: string): Promise<void> {
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
      if (record.overrideReason) journal.decision("override", record.overrideReason);
      const primary = session.bare ? await primaryCheckout() : undefined;
      if (primary) await checkPrimary(primary);
      update("rebasing", `Rebasing onto ${integration}.`);
      journal.timeline("land", `Rebasing onto ${integration}.`);
      const ref = `refs/heads/${integration}`;
      const tip = (await checked(git, ["rev-parse", "--verify", ref], session.worktree)).trim();
      const checkPatch = !session.bare && context.settings.exitPolicy === "review" && !record.overrideReason;
      const before = checkPatch ? await patchDigest(git, (await checked(git, ["merge-base", tip, "HEAD"], session.worktree)).trim(), session.worktree) : undefined;
      await rebase();
      if (before !== undefined && before !== await patchDigest(git, tip, session.worktree)) {
        throw new Error("The rebase changed the patch. Review the rebased head again with build_review, then choose Land.");
      }
      const head = (await checked(git, ["rev-parse", "HEAD"], session.worktree)).trim();
      let gateAttempt: string | undefined;
      if (!session.bare) {
        update("verifying", "Running the project's gate on the rebased head.");
        const gate = await verifyBuild(context, git, journal);
        gateAttempt = gate.attempt;
        if (gate.result === "fail") throw new Error(`Gate failed: ${gate.failed_command}; log: ${gate.log}`);
        if (gate.head !== head || (await checked(git, ["rev-parse", "HEAD"], session.worktree)).trim() !== head) {
          throw new Error("Build HEAD changed during verification. Choose Land again to verify the new head.");
        }
      }
      if ((await checked(git, ["rev-parse", "--verify", ref], session.worktree)).trim() !== tip) {
        throw new Error(`The base ${integration} moved. Choose Land again to rebase onto its new tip.`);
      }
      if (primary) await checkPrimary(primary);
      const baseCheckout = primary ?? (await checked(git, ["worktree", "list", "--porcelain", "-z"], session.worktree))
        .split("\0\0").map(entry => entry.split("\0")).find(entry => entry.includes(`branch ${ref}`))?.[0]?.slice("worktree ".length);
      update("landing", `Locally fast-forwarding ${integration} to ${head}.`);
      if (baseCheckout) await checked(git, ["merge", "--ff-only", head], baseCheckout);
      else await checked(git, ["fetch", ".", `${head}:${ref}`], session.worktree);
      journal.outcome({ state: "landed", at: new Date(now()).toISOString(), head, landed: head, base_branch: integration, override_reason: record.overrideReason });
      journal.timeline("land", `${integration}: ${head}`);
      update("landed", `Landed at ${head} on ${integration}.`, { ...record, landed: head, baseBranch: integration, gateAttempt });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      journal.timeline("exit", `failed: ${detail}`);
      update("failed", detail);
    } finally {
      try { await journal.refresh(); }
      catch (error) {
        update(state.phase, `${state.detail ?? ""}\nBuild record Git snapshot failed: ${String(error)}`);
      }
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
