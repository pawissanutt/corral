import { randomUUID } from "node:crypto";
import { lstatSync, realpathSync } from "node:fs";
import { isAbsolute } from "node:path";
import { taskDestinationLock } from "../host/worktree-writer.ts";
import { checked, type Run } from "./corral-cli.ts";
import { fastForward, PartialFastForward } from "./fast-forward.ts";
import { LandTracker, TAIL_FAILED } from "./land-progress.ts";
import { summarize } from "./git.ts";
import { ancestor, baseMoved, CandidateMoved, checkDestination, checkSource, checkTargetConfig, commit, landingDisabled, landingTarget, message, PreparationError,
  PREPARATION_NEEDED, rebaseCandidate, short, sourceBranch, type LandingTarget } from "./prepare.ts";
import type { BuildJournal } from "./record.ts";
import type { BuildGateResult } from "./record-types.ts";
import { proveReviewReplay, reviewCoversCandidate, reviewPolicy } from "./review-evidence.ts";
import { LIVE_CHILD_STATES, type BuildContext, type BuildSummary, type ExitChoice, type ExitControl, type ExitOption, type ExitRecord,
  type ExitState, type FleetSnapshot, type LandCandidate, type PreparedLanding } from "./types.ts";

/** Recovery metadata stays local to session.json, not the tracked build record. */
export interface ExitProgressRecord extends ExitRecord {
  readonly progress?: { readonly phase: ExitState["phase"]; readonly detail?: string };
}

export type GateProgressEvent = { readonly kind: "start"; readonly commands: readonly string[]; readonly log: string }
  | { readonly kind: "command"; readonly index: number }
  | { readonly kind: "tick" }
  | { readonly kind: "end"; readonly gate: BuildGateResult }
  | { readonly kind: "stop"; readonly detail: string };

export interface BuildExitControl extends ExitControl {
  requestFinish(): Promise<void>;
  /** Explicitly prepare for evidence or finish. The returned base is the immutable integration tip SHA. */
  prepare(): Promise<PreparedLanding | undefined>;
  preparationDetail(): string | undefined;
  /** Validate the pin without mutating Git. Evidence polling can omit cleanliness and child checks. */
  checkPrepared(candidate: PreparedLanding, options?: { readonly identityOnly?: boolean }): Promise<void>;
  gateProgress(candidate: PreparedLanding, event: GateProgressEvent): void;
  /** Display a choice without preparing. With null, offer no candidate even if an old pin exists. */
  open(candidate?: PreparedLanding | null, detail?: string): void;
  /** Withdraw an unanswered choice when the parent resumes working. */
  withdraw(): void;
  close(): void;
  busy(): boolean;
  choiceBusy(): boolean;
  /** A finish coordinator cannot open over a choice accepted while its evidence was pending. */
  choiceRevision(): number;
  summarySnapshot(): BuildSummary | undefined;
}

interface Pin {
  readonly candidate: PreparedLanding;
  readonly target: LandingTarget;
  readonly source: string;
}
interface Offer { readonly pin: Pin; readonly authorization: string; shown: boolean }

function checkoutIdentity(checkout: string) {
  if (!isAbsolute(checkout)) throw new Error("The landing checkout path must be absolute.");
  const path = realpathSync(checkout);
  const stat = lstatSync(path, { bigint: true });
  if (!stat.isDirectory()) throw new Error(`The landing checkout at ${checkout} is unavailable.`);
  return { path, device: stat.dev, inode: stat.ino };
}

export function createExit(options: {
  context: BuildContext; git: Run; fleet: () => FleetSnapshot; journal: BuildJournal;
  save: (record: ExitRecord) => void; changed: () => void; now?: () => number;
  evidenceBusy?: () => boolean;
  requestFinish?: () => Promise<void>;
}): BuildExitControl {
  const { context, git, journal } = options;
  const { session } = context;
  const cwd = session.worktree;
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const saved = session.exit as ExitProgressRecord | undefined;
  let state: ExitState = saved ? { record: saved, phase: saved.progress?.phase ?? (saved.landed ? "landed" : saved.choice === "keep" ? "kept" : "none"), detail: saved.progress?.detail } : { phase: "none" };
  if (["rebasing", "verifying", "repairing", "landing"].includes(state.phase)) {
    const inspection = state.phase === "landing" ? "Inspect the base branch, its checkout and index for a partial fast-forward before retrying." : "Inspect the worktree.";
    state = { ...state, phase: "failed", detail: `The previous exit stopped during ${state.phase}. ${inspection} ${PREPARATION_NEEDED}` };
  }
  let summary: BuildSummary | undefined;
  let summaryEpoch = 0;
  let summaryRead = 0;
  let prepared: Pin | undefined;
  let preparationDetail: string | undefined;
  let candidateStatus: LandCandidate | undefined;
  let offered: Offer | undefined;
  let offerDetail: string | undefined;
  let preparing: Promise<PreparedLanding | undefined> | undefined;
  let choiceRevision = 0;
  let withdrawal = 0;
  let inFlight: { readonly choice: ExitChoice; readonly authorization?: string; readonly result: Promise<ExitState> } | undefined;
  let steps: LandTracker | undefined;
  let trackedPin: Pin | undefined;
  let gateFlight: { readonly pin: Pin; readonly phase: ExitState["phase"]; readonly commands: readonly string[]; readonly tracker: LandTracker } | undefined;
  let closed = false;

  function changed() {
    if (closed) return;
    try { options.changed(); } catch { /* Observers do not own exit execution. */ }
    for (const listener of [...listeners]) { try { listener(); } catch { /* Observers do not own exit execution. */ } }
  }
  function update(phase: ExitState["phase"], detail: string, record = state.record, save = false) {
    state = { phase, detail, ...(record ? { record } : {}) };
    if (save && record) options.save({ ...record, progress: { phase, detail } } as ExitProgressRecord);
    changed();
  }
  async function readSummary(base = session.base): Promise<BuildSummary> {
    return session.bare && session.repository === false
      ? { branch: null, base: null, head: null, commits: 0, files: 0, added: 0, removed: 0, dirty: false, slices: [], outside: [] }
      : summarize(git, { ...session, base }, options.fleet());
  }
  async function getSummary() {
    const epoch = summaryEpoch, read = ++summaryRead;
    const result = await readSummary(offered?.pin.candidate.base);
    // A passive read started before preparation must not overwrite its final snapshot.
    if (epoch === summaryEpoch && read === summaryRead && !preparing && !inFlight) { summary = result; changed(); }
    return result;
  }
  const hasLiveChildren = () => options.fleet().children.some(child => LIVE_CHILD_STATES.includes(child.state));
  function checkAvailable() {
    if (closed) throw new Error("The session closed.");
    const disabled = landingDisabled(context);
    if (disabled) throw new Error(disabled);
    if (hasLiveChildren()) throw new Error("Wait for all live children before preparing to land.");
  }
  function pinStatus(pin: Pin): LandCandidate {
    return { state: "ready", base: pin.target.name, baseTip: pin.candidate.base, head: pin.candidate.head, source: pin.source };
  }
  function reviewReason(pin: Pin): string | undefined {
    if (session.bare || context.settings.exitPolicy !== "review") return;
    const review = journal.latestReview();
    if (!review || review.verdict !== "approve" || !reviewCoversCandidate(review, pin.candidate, journal)
        || review.findings.some(finding => finding.severity !== "minor" && (!finding.resolution || finding.resolution.state === "open"))) {
      return "No approving review covers the prepared head with all blocking findings resolved. Give a reason to land anyway.";
    }
  }
  function gateReceipt(pin: Pin) {
    return session.bare ? undefined : journal.passedGate(pin.candidate.head, pin.target.gateInputFingerprint);
  }
  function evidenceStatus(pin: Pin): LandCandidate {
    const status = pinStatus(pin);
    const passed = gateReceipt(pin);
    if (pin.target.commands.length && !passed) {
      const latest = journal.latestGate();
      const action = `build_verify on the prepared head ${short(pin.candidate.head)} before landing.`;
      return latest?.result === "fail" && latest.head === pin.candidate.head && latest.input_fingerprint === pin.target.gateInputFingerprint
        ? { ...status, state: "gate_failed", gateAttempt: latest.attempt,
          detail: `Gate ${latest.attempt} failed${latest.failed_command ? `: ${latest.failed_command}` : ""}. Fix the failure, then run ${action}` }
        : { ...status, state: "needs_gate", detail: `Run ${action}` };
    }
    const needsReason = reviewReason(pin);
    return { ...status, ...(needsReason ? { state: "needs_review" as const, detail: needsReason } : {}),
      ...(passed ? { gateAttempt: passed.attempt } : {}) };
  }
  function visibleCandidate(): LandCandidate | undefined {
    if (!preparing && prepared && candidateStatus?.state === "ready" && (!offered || offered.pin === prepared)) {
      if (!inFlight && (gateFlight || options.evidenceBusy?.())) return { ...pinStatus(prepared), state: "preparing", detail: "Collecting verification and review on the prepared candidate." };
      if (["none", "pending"].includes(state.phase)) return { ...candidateStatus, ...evidenceStatus(prepared) };
    }
    return candidateStatus;
  }
  function view(): ExitState {
    const candidate = visibleCandidate();
    return { ...state, ...(steps ? { land: steps.view() } : {}), ...(candidate ? { candidate } : {}) };
  }
  function choices(): readonly ExitOption[] {
    const offer = offered;
    const busy = preparing ? "Landing preparation is in progress." : gateFlight || options.evidenceBusy?.() ? "Wait for the current build evidence to finish." : undefined;
    const blocked = landingDisabled(context)
      ?? busy
      ?? (hasLiveChildren() ? "Wait for all live children before landing." : undefined)
      ?? (summary?.dirty ? "Commit or stash uncommitted build changes before landing." : undefined)
      ?? (!offer ? offerDetail ?? preparationDetail ?? PREPARATION_NEEDED : undefined)
      ?? (offer && offer.pin !== prepared ? PREPARATION_NEEDED : undefined);
    const status = offer && !blocked ? evidenceStatus(offer.pin) : undefined;
    const unavailable = blocked ?? (status?.state === "needs_gate" || status?.state === "gate_failed" ? status.detail : undefined);
    // Seeing a disabled option (e.g. a missing receipt) never publishes its authorization.
    if (offer && !unavailable) offer.shown = true;
    return [
      { choice: "keep", label: "Keep", detail: "Keep this worktree and branch and record the result.", unavailable: busy },
      ...(session.bare && session.repository === false ? [] : [{ choice: "land" as const, label: "Land",
        detail: offer ? `Fast-forward ${offer.pin.target.name} to the prepared candidate.` : "Fast-forward the base branch to a prepared candidate.",
        unavailable, ...(offer && !unavailable ? { authorization: offer.authorization, needsReason: reviewReason(offer.pin) } : {}) }]),
      { choice: "iterate", label: "Iterate", detail: "Return to the session and continue working.", unavailable: busy },
    ];
  }

  async function checkIdentity(pin: Pin, run = git): Promise<void> {
    if (closed) throw new Error("The session closed.");
    if (pin !== prepared) throw new PreparationError(`The prepared landing candidate is no longer current. ${PREPARATION_NEEDED}`);
    const { candidate, target } = pin;
    const current = await landingTarget(run, context);
    if (target.ref !== current.ref) throw new PreparationError(`The configured base branch changed from ${target.name} to ${current.name}. ${PREPARATION_NEEDED}`);
    if (target.gateInputFingerprint !== current.gateInputFingerprint) throw new PreparationError(`The configured gate inputs changed. ${PREPARATION_NEEDED} Run build_verify for the new commands.`);
    const tip = await commit(run, target.ref, cwd);
    if (tip !== candidate.base) throw baseMoved(target, candidate.base, tip);
    if (await sourceBranch(run, cwd) !== candidate.branch) throw new PreparationError(`The build branch changed since preparation. ${PREPARATION_NEEDED}`);
    if (await commit(run, "HEAD", cwd) !== candidate.head) throw new CandidateMoved(`Build HEAD changed since preparation. ${PREPARATION_NEEDED}`);
    if (pin !== prepared) throw new PreparationError(`The prepared landing candidate is no longer current. ${PREPARATION_NEEDED}`);
  }
  function checkReviewPolicy(pin: Pin): void {
    if (reviewPolicy(context, pin.target, pin.candidate.branch)?.scope !== pin.candidate.reviewScope) {
      throw new PreparationError(`The configured review inputs changed. ${PREPARATION_NEEDED}`);
    }
  }
  function inspectDestination(target: LandingTarget, base: string, head?: string, run = git) {
    // Even diagnostic Git commands can refresh the destination's index unless opted out.
    return checkDestination((argv, options) => run(argv, { ...options, env: { ...options.env, GIT_OPTIONAL_LOCKS: "0" } }), context, target, base, head);
  }
  async function checkPin(pin: Pin, run = git): Promise<string | undefined> {
    checkAvailable();
    await checkSource(run, context);
    const { candidate, target } = pin;
    checkTargetConfig(context, target);
    const checkout = await inspectDestination(target, candidate.base, candidate.head, run);
    await checkIdentity(pin, run);
    checkReviewPolicy(pin);
    checkAvailable();
    return checkout;
  }

  async function prepare(): Promise<PreparedLanding | undefined> {
    const previousPhase = state.phase;
    const previousRecord = state.record;
    const startedAtWithdrawal = withdrawal;
    const restorePhase = () => previousPhase === "pending" && withdrawal !== startedAtWithdrawal ? "none" : previousPhase;
    let status: LandCandidate = { state: "preparing" };
    let rebasing = false;
    let tracker: LandTracker | undefined;
    const publish = () => { candidateStatus = status; changed(); };
    preparationDetail = undefined;
    try {
      checkAvailable();
      if (options.evidenceBusy?.()) throw new Error("Wait for the current build evidence to finish before preparing.");
      publish();
      await checkSource(git, context);
      summary = await readSummary();
      const target = await landingTarget(git, context);
      const branch = await sourceBranch(git, cwd);
      const policy = reviewPolicy(context, target, branch);
      if (branch === target.ref) throw new Error(session.bare ? "The session is on its starting branch; use a separate branch." : "The session is on the project's base branch; use a separate branch.");
      if (!session.here && session.branch && branch !== `refs/heads/${session.branch}`) throw new Error(`Switch the build worktree back to ${session.branch} before preparing.`);
      if (!summary.base || !summary.commits) throw new Error("There are no commits beyond the session base.");
      const source = await commit(git, "HEAD", cwd);
      if (summary.head !== source) throw new Error("Build HEAD changed while reading the branch summary. Try preparing again.");
      const base = await commit(git, target.ref, cwd);
      status = { ...status, base: target.name, baseTip: base, source };
      const alreadyBased = await ancestor(git, base, source, cwd);
      if (source === base || !alreadyBased && await ancestor(git, source, base, cwd)) {
        throw new Error("There are no candidate commits beyond the integration tip.");
      }
      await inspectDestination(target, base, alreadyBased ? source : undefined);
      let head = source;
      if (!alreadyBased) {
        const fork = await git(["merge-base", base, source], { cwd });
        if (fork.code !== 0 || !fork.stdout.trim()) {
          throw new Error("Cannot find a common ancestor of the integration tip and build head. Inspect the branches before preparing.");
        }
        rebasing = true;
        // A pending offer must never be rebound to the result of this rebase.
        prepared = undefined;
        offered = undefined;
        trackedPin = undefined;
        tracker = steps = new LandTracker(target.name, !session.bare, now);
        tracker.begin("rebase");
        candidateStatus = status;
        update("rebasing", `Preparing the candidate on ${target.name} (${short(base)}).`);
        journal.timeline("land", `Preparing onto ${target.name} at ${base}.`);
        await checkSource(git, context);
        if (await sourceBranch(git, cwd) !== branch || await commit(git, "HEAD", cwd) !== source) throw new Error("The build branch changed during preparation. Try again.");
        checkAvailable();
        await rebaseCandidate(git, cwd, base, source);
        head = await commit(git, "HEAD", cwd);
        tracker.done("rebase", short(head));
      }
      status = { ...status, head };
      if (head === base) throw new Error("There are no candidate commits beyond the integration tip.");
      if (!await ancestor(git, base, head, cwd)) throw new Error("The prepared candidate is not a fast-forward of the captured base.");
      summary = await readSummary(base);
      if (summary.head !== head) throw new Error("Build HEAD changed while reading the prepared candidate. Try preparing again.");
      const current = prepared;
      const pin: Pin = current?.candidate.head === head && current.candidate.base === base && current.candidate.branch === branch
        && current.candidate.reviewScope === policy?.scope
        && current.target.ref === target.ref && current.target.configuredName === target.configuredName && current.target.gateInputFingerprint === target.gateInputFingerprint
        ? current : { candidate: Object.freeze({ head, base, branch, reviewScope: policy?.scope }), target, source };
      if (offered?.pin !== pin) offered = undefined;
      if (tracker) trackedPin = pin;
      else if (trackedPin !== pin) { steps = undefined; trackedPin = undefined; }
      prepared = pin;
      const review = rebasing && policy ? journal.latestReview() : undefined;
      let carriedRound: number | undefined;
      if (review?.evidence && policy && await proveReviewReplay(git, cwd, review, source, pin.candidate, policy.neutralDocs, journal.reviewContext().fingerprint)) {
        // Certify the replay independently of the live tip, which can already
        // have advanced again. Preparation still requires a strictly current pin.
        await checkSource(git, context);
        if (await sourceBranch(git, cwd) !== branch || await commit(git, "HEAD", cwd) !== head) {
          throw new CandidateMoved(`The build branch changed during preparation. ${PREPARATION_NEEDED}`);
        }
        checkTargetConfig(context, target);
        checkReviewPolicy(pin);
        if (review.evidence.context === journal.reviewContext().fingerprint && journal.carryReview(review, { base, head })) {
          carriedRound = review.round;
        }
      }
      await checkPin(pin);
      const detail = head === source
        ? `Prepared ${short(head)} on ${target.name} (${short(base)}).`
        : `Rebased ${short(source)} to ${short(head)} on ${target.name} (${short(base)}).${carriedRound ? ` Carried approval from review round ${carriedRound}.` : ""}`;
      candidateStatus = { ...pinStatus(pin), detail };
      if (head !== source) journal.timeline("land", detail);
      if (rebasing) update(restorePhase(), detail, previousRecord);
      return pin.candidate;
    } catch (error) {
      prepared = undefined;
      offered = undefined;
      preparationDetail = message(error);
      candidateStatus = { ...status, state: "failed", ...(error instanceof PreparationError ? error.status : {}), detail: preparationDetail };
      tracker?.fail(preparationDetail);
      if (rebasing) {
        journal.timeline("land", `Preparation stopped: ${preparationDetail}`);
        update(restorePhase() === "pending" ? "pending" : "failed", preparationDetail, previousRecord);
      }
      return undefined;
    }
  }

  function gateProgress(candidate: PreparedLanding, event: GateProgressEvent): void {
    if (closed) return;
    if (event.kind === "start") {
      const pin = prepared;
      if (!pin || pin.candidate !== candidate || inFlight || preparing || gateFlight) return;
      trackedPin = pin;
      const tracker = steps = new LandTracker(pin.target.name, true, now);
      tracker.done("rebase", `prepared ${short(candidate.head)}`);
      tracker.begin("gate");
      tracker.setGate({ log: event.log, of: event.commands.length });
      gateFlight = { pin, phase: state.phase, commands: event.commands, tracker };
      update("verifying", `Running the project's gate on prepared head ${short(candidate.head)}.`);
      return;
    }
    const flight = gateFlight;
    if (!flight || flight.pin.candidate !== candidate) return;
    const { tracker } = flight;
    if (event.kind === "tick") { if (tracker.refreshTail()) changed(); return; }
    if (event.kind === "command") {
      tracker.setGate({ command: flight.commands[event.index], index: event.index + 1 });
      changed();
      return;
    }
    gateFlight = undefined;
    if (event.kind === "stop") {
      tracker.clearTail();
      tracker.fail("stopped", "gate");
      update(flight.phase, event.detail);
      return;
    }
    const { gate } = event;
    tracker.setGate({ attempt: gate.attempt });
    if (gate.result === "fail") {
      if (gate.failed_command) tracker.setGate({ command: gate.failed_command });
      tracker.refreshTail(TAIL_FAILED);
      tracker.fail(`${gate.attempt} failed`, "gate");
    } else {
      tracker.clearTail();
      tracker.done("gate", gate.result === "skipped" ? "no gate commands" : `${gate.attempt} passed`);
    }
    update(flight.phase, `Gate ${gate.attempt}: ${gate.result}.`);
  }

  function refuse(detail: string): ExitState { return { ...view(), phase: "failed", detail }; }
  async function execute(choice: ExitChoice, offer: Offer | undefined, overrideReason: string | undefined, moved: () => void): Promise<ExitState> {
    const record: ExitRecord = { choice, at: new Date(now()).toISOString(), ...(overrideReason ? { overrideReason } : {}) };
    try {
      steps = undefined;
      trackedPin = undefined;
      offered = undefined;
      journal.decision("exit", choice);
      journal.timeline("exit", choice);
      if (choice === "iterate") { update("none", "The build session remains open for iteration.", state.record, true); }
      else if (choice === "keep") {
        try { summary = await readSummary(); } catch { /* Keeping does not require a Git head. */ }
        journal.outcome({ state: "kept", at: record.at, head: summary?.head ?? undefined });
        journal.timeline("exit", "kept");
        update("kept", "The worktree and branch are kept.", record, true);
      } else if (offer) {
        const pin = offer.pin;
        const { candidate, target } = pin;
        const progress = steps = new LandTracker(target.name, !session.bare, now);
        progress.done("rebase", `prepared ${short(candidate.head)}`);
        progress.begin("land");
        candidateStatus = evidenceStatus(pin);
        const checkout = await checkPin(pin);
        const workspace = session.workspaceRoot;
        const destination = checkout ? checkoutIdentity(checkout) : undefined;
        const authorize = () => {
          if (options.evidenceBusy?.()) throw new Error("Wait for the current build evidence to finish.");
          const needsReason = reviewReason(pin);
          if (needsReason && !overrideReason) throw new Error(needsReason);
          const passed = gateReceipt(pin);
          if (target.commands.length && !passed) throw new Error(evidenceStatus(pin).detail);
          return passed;
        };
        const passed = authorize();
        if (!session.bare) progress.done("gate", passed ? `${passed.attempt} passed earlier on this commit` : "no gate commands");
        if (passed) progress.setGate({ attempt: passed.attempt, log: passed.log });
        if (overrideReason) journal.decision("override", overrideReason);
        candidateStatus = { ...pinStatus(pin), ...(passed ? { gateAttempt: passed.attempt } : {}) };
        update("landing", `Fast-forwarding ${target.name} to ${short(candidate.head)}.`, record, true);
        // The Build host's lifetime writer lock covers its source, not this checkout.
        const writer = destination ? await taskDestinationLock(destination.path, context.corral, workspace) : undefined;
        const destinationChanged = () => new PreparationError(`The checkout of ${target.name} changed during landing. ${PREPARATION_NEEDED}`);
        const checkDestinationWriter = () => {
          if (destination && checkout) {
            const current = checkoutIdentity(checkout);
            if (current.path !== destination.path || current.device !== destination.device || current.inode !== destination.inode) {
              throw destinationChanged();
            }
          }
          writer?.check();
        };
        const landingGit: Run = writer ? (argv, options) => git(argv, { ...options, leaseFds: [...(options.leaseFds ?? []), writer.fd] }) : git;
        const leaseFds = [context.writerFd, writer?.fd].filter(fd => fd !== undefined);
        let receipt: ReturnType<typeof authorize>;
        try {
          // Do not silently switch to another checkout (or a ref-only update) after admission.
          if (await checkPin(pin, landingGit) !== checkout) throw destinationChanged();
          checkDestinationWriter();
          await fastForward(landingGit, { source: cwd, checkout: destination?.path ?? checkout, ref: target.ref, base: candidate.base, head: candidate.head, leaseFds, async beforeWrite() {
            // Destination locks and index staging can take time. Recheck the source
            // against the offer here without reacquiring any destination locks.
            await checkSource(landingGit, context);
            await checkIdentity(pin, landingGit);
            if (!checkout && await inspectDestination(target, candidate.base, candidate.head, landingGit)) {
              throw new Error(`The base ${target.name} is now checked out. ${PREPARATION_NEEDED}`);
            }
            if (checkout && session.bare && (await checked(landingGit, ["status", "--porcelain", "--untracked-files=no", "--ignore-submodules=none"], checkout)).trim()) {
              throw new Error("The repository's primary checkout has tracked changes; commit or stash them before landing.");
            }
            checkAvailable();
            checkTargetConfig(context, target);
            checkReviewPolicy(pin);
            checkDestinationWriter();
            receipt = authorize();
          } }).catch(async error => {
            // Do not hide partial-checkout diagnostics or automatically reprepare over one.
            if (error instanceof PartialFastForward) throw error;
            const actual = await commit(git, target.ref, cwd).catch(() => undefined);
            if (actual && actual !== candidate.base) throw baseMoved(target, candidate.base, actual);
            throw error;
          });
        } finally {
          writer?.close();
        }
        progress.done("land");
        prepared = undefined;
        journal.outcome({ state: "landed", at: new Date(now()).toISOString(), head: candidate.head, landed: candidate.head,
          base_branch: target.name, override_reason: overrideReason });
        journal.timeline("land", `${target.name}: ${candidate.head}`);
        update("landed", "Landed.", { ...record, landed: candidate.head, baseBranch: target.name, gateAttempt: receipt?.attempt }, true);
      }
    } catch (error) {
      const retry = choice === "land" && error instanceof CandidateMoved && options.requestFinish;
      const detail = retry ? `${message(error).replace(` ${PREPARATION_NEEDED}`, "")} Checking for a fresh landing candidate; a new choice will be required.` : message(error);
      if (retry) moved();
      if (choice === "land") {
        candidateStatus = { ...candidateStatus, state: "failed", ...(error instanceof PreparationError ? error.status : {}), detail };
        preparationDetail = detail;
        prepared = undefined;
        steps?.fail(candidateStatus.state === "base_moved" ? "base moved" : undefined, "land");
      }
      journal.timeline("exit", `failed: ${detail}`);
      update("failed", detail, record, true);
    } finally {
      try { await journal.refresh(); }
      catch (error) { update(state.phase, `${state.detail ?? ""}\nBuild record Git snapshot failed: ${message(error)}`); }
    }
    return view();
  }

  const control: BuildExitControl = {
    options: choices, summary: getSummary, state: view,
    requestFinish: options.requestFinish ?? (async () => { control.open(); }),
    busy: () => !!(preparing || inFlight || gateFlight), choiceBusy: () => !!inFlight, choiceRevision: () => choiceRevision, summarySnapshot: () => summary, gateProgress,
    prepare() {
      if (preparing) return preparing;
      if (closed || inFlight || gateFlight || options.evidenceBusy?.()) {
        preparationDetail = closed ? "The session closed." : inFlight ? "An exit choice is already in progress." : "Wait for the current build evidence to finish before preparing.";
        return Promise.resolve(undefined);
      }
      ++summaryEpoch;
      preparing = Promise.resolve().then(prepare).finally(() => { preparing = undefined; changed(); });
      return preparing;
    },
    preparationDetail: () => preparationDetail,
    async checkPrepared(candidate, opts) {
      const pin = prepared;
      if (!pin || pin.candidate !== candidate) throw new PreparationError(`The prepared landing candidate is no longer current. ${PREPARATION_NEEDED}`);
      try {
        if (opts?.identityOnly) await checkIdentity(pin);
        else await checkPin(pin);
      } catch (error) {
        // Evidence can invalidate the in-memory pin, but never repairs it or mutates Git.
        if (prepared === pin && !preparing && !inFlight && !closed) {
          prepared = undefined;
          offered = undefined;
          preparationDetail = message(error);
          candidateStatus = { ...pinStatus(pin), state: "failed", ...(error instanceof PreparationError ? error.status : {}), detail: preparationDetail };
          changed();
        }
        throw error;
      }
    },
    choose(choice, opts) {
      if (closed || state.phase === "landed" && choice !== "iterate") return Promise.resolve(view());
      if (inFlight) return inFlight.choice === choice && inFlight.authorization === opts?.authorization
        ? inFlight.result : Promise.resolve(refuse("An exit choice is already in progress."));
      if (preparing) return Promise.resolve(refuse("Landing preparation is in progress. Wait for it to finish."));
      if (gateFlight || options.evidenceBusy?.()) return Promise.resolve(refuse("Wait for the current build evidence to finish."));
      const offer = offered;
      if (choice === "land") {
        if (state.phase !== "pending" || !offer?.shown || opts?.authorization !== offer.authorization || offer.pin !== prepared) {
          return Promise.resolve(refuse(offerDetail ?? preparationDetail ?? `That Land choice is no longer offered. ${PREPARATION_NEEDED}`));
        }
        const reason = reviewReason(offer.pin);
        if (reason && !opts.overrideReason?.trim()) return Promise.resolve(refuse(reason));
      }
      let retry = false;
      ++choiceRevision;
      ++summaryEpoch;
      const result = Promise.resolve().then(() => execute(choice, offer, opts?.overrideReason?.trim(), () => { retry = true; })).finally(() => {
        inFlight = undefined;
        changed();
        const failed = state;
        if (retry) setTimeout(() => {
          if (!closed && state === failed && failed.phase === "failed" && !inFlight && !preparing && !hasLiveChildren() && !options.evidenceBusy?.()) {
            void Promise.resolve().then(() => options.requestFinish?.()).catch(error => {
              if (state === failed && !closed) update("failed", `${failed.detail}\nCould not prepare a fresh candidate: ${message(error)}`);
            });
          }
        }, 0);
      });
      inFlight = { choice, authorization: opts?.authorization, result };
      return result;
    },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    open(candidate, detail) {
      if (closed || preparing || inFlight || gateFlight) return;
      if (state.phase === "pending" && candidate === undefined) return;
      const pin = candidate === null || candidate !== undefined && candidate !== prepared?.candidate ? undefined : prepared;
      offerDetail = pin ? undefined : detail;
      if (!pin) offered = undefined;
      else if (pin !== offered?.pin) offered = { pin, authorization: randomUUID(), shown: false };
      if (!prepared && candidateStatus?.state === "ready") candidateStatus = undefined;
      const previouslyTracked = !pin && trackedPin && candidateStatus?.head === trackedPin.candidate.head && candidateStatus.baseTip === trackedPin.candidate.base;
      if (!(pin && trackedPin === pin) && !previouslyTracked && !(candidateStatus?.state === "conflicts")) { steps = undefined; trackedPin = undefined; }
      update("pending", detail ?? "Choose Keep, Land or Iterate.");
      void getSummary().catch(() => { /* A summary failure cannot prevent Keep or Iterate. */ });
    },
    withdraw() {
      if (closed || inFlight) return;
      ++withdrawal;
      offered = undefined;
      offerDetail = undefined;
      if (gateFlight?.phase === "pending") gateFlight = { ...gateFlight, phase: "none" };
      if (state.phase === "pending") {
        ++summaryEpoch;
        update("none", "The parent resumed work; the previous exit choice is no longer offered.");
      }
    },
    close() { closed = true; listeners.clear(); },
  };
  return control;
}
