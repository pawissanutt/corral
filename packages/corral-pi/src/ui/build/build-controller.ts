// What a build session's UI decides on its own: which docked dialog shows and when, and who the composer's Enter
// reaches. Views stay pure; this keeps the episodes. With the session's dialog registry, every docked decision goes
// through the registry's card for it, which the app answers too; the host records the outcome in the conversation.
import type {
  BoardPlan, BuildPanelState, BuildSummary, BuildUi, ChildSnapshot, ExitControl, ExitOption, ExitPhase, FleetSnapshot,
} from "../../build/types.ts";
import { LIVE_CHILD_STATES } from "../../build/types.ts";
import { BridgeRefusal } from "../../bridge/protocol.ts";
import type { AnswerStatus, CardAnswer, CardEntry } from "../../host/remote/cards.ts";
import { optionId } from "../../host/remote/cards.ts";
import type { DialogResolution, ExistingDialog, RemoteDialogs } from "../../host/remote/dialogs.ts";
import type { ChildMark, MarkSink } from "../../host/transcript-marks.ts";
import { PANEL_BLUE, PANEL_WARNING } from "../panels-view.ts";
import { exitDialog, planDialog, questionDialog } from "./build-dialogs.ts";
import type { PreviewState } from "./build-dialogs.ts";
import { finishEntry } from "./build-entries.ts";
import type { BuildEntry } from "./build-entries.ts";
import { CHOICE_GUARD_MS, ChoiceDock } from "./choice-dock.ts";
import type { ChoiceDialog, ChoiceOption } from "./choice-dock.ts";
import { childHue } from "./contract.ts";
import type { BuildCardSource, ComposerTarget } from "./contract.ts";
import { steerable } from "./fleet-rows.ts";
import { fastForward } from "./land-steps.ts";

export interface BuildControllerHost {
  readonly ui: BuildUi;
  /** `snapshot.build`. */
  state(): BuildPanelState | undefined;
  /** The parent streams. */
  streaming(): boolean;
  /** A Pi dialog or overlay is open. */
  overlay(): boolean;
  draft(): string;
  setDraft(text: string): void;
  focusComposer(): void;
  /** Logical focus "choice". */
  focusDock(): void;
  notice(text: string, tone: "text" | "success" | "warning" | "error"): void;
  /** Appends a conversation entry; the panels post it when the parent is idle. */
  post(entry: BuildEntry<unknown>): void;
  /** Where a child's question, the owner's answer and a steer are recorded as transcript marks; absent records nothing. */
  readonly marks?: MarkSink;
  /**
   * The session's dialog registry, read at each use. Present while a terminal is attached: the exit choice, a board
   * plan and a child's question are decided through its card, which also delivers the answer and records it, and an
   * answer from the app closes the dock. Absent (a headless session, tests): the controller never watches or answers
   * a card, and decisions go straight to the build controls.
   */
  readonly dialogs?: RemoteDialogs;
  showBoard(): void;
  openChild(id: string): void;
  color(hue: number): string;
  now(): number;
  changed(): void;
}

type Shown = { readonly kind: "exit" } | { readonly kind: "question"; readonly id: string; readonly askedAt: number } | { readonly kind: "plan" };

const PLAN_DRAFT = "Split the remaining work into board tasks and propose them with board_plan.";
/**
 * Why Finish waits while an exit runs. Kept and landed are outcomes, not the end of the session: Finish opens the
 * choice again, and the host offers Land only for commits it has not landed.
 */
const DONE_REASON: Partial<Record<ExitPhase, string>> = {
  rebasing: "this session is already landing", verifying: "this session is already landing",
  repairing: "this session is already landing", landing: "this session is already landing",
};
/** The composer target while Enter gives the owner's reason to land. */
const REASON_TARGET: ComposerTarget = { id: "exit", answer: true };

const questionKey = (id: string, askedAt: number) => `${id}:${askedAt}`;
/** An error's words for a notice; a registry refusal gives its reason without its code. */
const message = (error: unknown) => error instanceof BridgeRefusal ? error.reason : error instanceof Error ? error.message : String(error);

/** The host's card for child `child`'s question asked at `askedAt`: kind, child and the time it was asked. */
export function childQuestionCard(child: string, askedAt: number): (card: Readonly<CardEntry["card"]>) => boolean {
  const opened = new Date(askedAt).toISOString();
  return card => card.kind === "child_question" && card.child === child && card.opened_at === opened;
}

/**
 * Which registry card is `shown`'s decision: the exit (the one card `pinned`, once the dock saw it), the board plan,
 * or this child's question at its `askedAt`. The host gives a new candidate a new exit card, so a pinned one never
 * answers for a candidate the owner was not shown.
 */
function cardOf(shown: Shown, pinned?: string): (card: Readonly<CardEntry["card"]>) => boolean {
  if (shown.kind === "exit") return pinned === undefined ? card => card.kind === "exit" : card => card.kind === "exit" && card.id === pinned;
  if (shown.kind === "plan") return card => card.kind === "board_plan";
  return childQuestionCard(shown.id, shown.askedAt);
}

export class BuildController implements BuildCardSource {
  readonly dock: ChoiceDock;
  private shown: Shown | undefined;
  private lastPhase: ExitPhase;
  private lastTarget: string;
  /** Exit episodes: one per entry into `pending`. */
  private episode = 0;
  private choosing = false;
  /** An explicit Finish waits for the host to prepare its candidate. */
  private finishing = false;
  private summaryValue: BuildSummary | undefined;
  private latest: string | undefined;
  private finishes = 0;
  /** Question episodes `(child id, askedAt)` already noticed, dismissed with Esc, or answered from the dialog. */
  private readonly seen = new Set<string>();
  private readonly dismissed = new Set<string>();
  private readonly answered = new Set<string>();
  /** The proposed plan the current plan episode belongs to. */
  private tracked: BoardPlan | undefined;
  private planEpisode = 0;
  private preview: PreviewState = { state: "loading" };
  private filing = false;
  private fileError: string | undefined;
  private answerFor: string | undefined;
  private steering = false;
  /**
   * The exit episode whose Land waits for the owner's reason in the composer, with the Land option and exit card
   * the owner picked: the reason lands that candidate or nothing.
   */
  private reasonFor: { readonly episode: number; readonly option: ExitOption; readonly card: string | undefined } | undefined;
  /** The exit card and Land ticket the docked exit choice was shown with. */
  private exitCard: string | undefined;
  private exitTicket: string | undefined;
  /** When the exit's current phase began, as `sync` saw it change. */
  private phaseAt: number | undefined;
  private readonly watchers = new Set<{ readonly invalidate: () => void }>();
  /** The docked decision's card in the registry, watched while it is docked so an answer from the app closes it. */
  private watching: ExistingDialog | undefined;
  private disposed = false;

  constructor(private readonly host: BuildControllerHost) {
    const phase = host.ui.exit.state().phase;
    // A pending exit found at start is a new episode for the first sync; a settled one was reported before.
    this.lastPhase = phase === "pending" ? "none" : phase;
    this.lastTarget = host.ui.target();
    this.dock = new ChoiceDock({
      dialog: () => this.dialog(),
      pick: index => this.pick(index),
      refuse: option => this.refuse(option),
      dismiss: () => this.dismiss(),
      click: event => event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl && !host.overlay(),
      focus: () => { this.dock.focused = true; host.focusDock(); },
      changed: () => host.changed(),
    });
  }

  /** Every snapshot: track exit/plan/question episodes, auto-open, fetch the summary once per exit episode, start previews, notice a target fallback. */
  sync(): void {
    if (this.disposed) return;
    const ui = this.host.ui, fleet = ui.fleet.snapshot();
    const target = ui.target();
    if (target !== this.lastTarget) {
      const previous = this.lastTarget;
      this.lastTarget = target;
      const live = fleet.children.some(child => child.id === previous && LIVE_CHILD_STATES.includes(child.state));
      if (target === "parent" && previous !== "parent" && !live) this.host.notice(`${previous} finished · messages go to the parent again`, "text");
    }
    if (this.answerFor && (target !== this.answerFor || !fleet.children.some(child => child.id === this.answerFor && child.state === "needs"))) {
      this.answerFor = undefined;
    }

    const arrivals: Shown[] = [];
    const phase = ui.exit.state().phase;
    if (phase !== this.lastPhase) {
      this.lastPhase = phase;
      this.phaseAt = this.host.now();
      if (phase !== "pending") this.reasonFor = undefined;
      if (phase === "pending") {
        this.episode++;
        this.summaryValue = undefined;
        this.readSummary(this.episode);
        arrivals.push({ kind: "exit" });
      } else if (this.shown?.kind === "exit") this.close();
    }

    const asking = this.questions(fleet);
    for (const child of asking) {
      const key = questionKey(child.id, child.question!.askedAt);
      if (!this.seen.has(key)) {
        this.seen.add(key);
        arrivals.push({ kind: "question", id: child.id, askedAt: child.question!.askedAt });
        this.mark("asked", child.id, child.question!.question, child.question!.askedAt);
      }
    }
    const shown = this.shown;
    if (shown?.kind === "question" && !asking.some(child => child.id === shown.id && child.question!.askedAt === shown.askedAt)) {
      this.close();
      this.next(fleet);
    }

    const proposed = ui.board?.proposed();
    if (proposed !== this.tracked) {
      this.track(proposed);
      if (proposed) arrivals.push({ kind: "plan" });
      else if (this.shown?.kind === "plan") this.close();
    }

    const first = arrivals[0];
    if (first && !this.shown && !this.host.overlay() && !this.host.draft().trim()) this.show(first, true);
    // An arrival that did not dock (a draft, an overlay or another dialog is in the way) still says so.
    for (const arrival of arrivals) {
      if (this.shown === arrival) continue;
      if (arrival.kind === "question") this.host.notice(`${arrival.id} asks a question`, "warning");
      else if (arrival.kind === "exit") this.host.notice("The exit choice is waiting for you", "warning");
      else this.host.notice("A plan is waiting for you", "warning");
    }
    for (const watcher of [...this.watchers]) watcher.invalidate();
  }

  /** The docked dialog, or undefined when none is shown or what it asked about is gone. */
  dialog(): ChoiceDialog | undefined {
    const shown = this.shown, ui = this.host.ui;
    if (!shown) return undefined;
    if (shown.kind === "exit") {
      const state = this.host.state(), exit = ui.exit.state();
      if (!state || exit.phase !== "pending") return undefined;
      const live = ui.fleet.snapshot().children.filter(child => LIVE_CHILD_STATES.includes(child.state)).length;
      return exitDialog(state, ui.exit.options(), this.episode, live, exit.candidate);
    }
    if (shown.kind === "question") {
      const fleet = ui.fleet.snapshot(), child = fleet.children.find(entry => entry.id === shown.id);
      if (!child?.question || child.state !== "needs" || child.question.askedAt !== shown.askedAt) return undefined;
      return questionDialog(child, this.host.color(childHue(fleet, child.id)));
    }
    return this.tracked ? planDialog(this.tracked, this.preview, this.planEpisode, this.fileError) : undefined;
  }

  /** Docks a dialog the owner asked for; false plus a notice when there is nothing to open. */
  open(kind: "exit" | "question" | "plan", id?: string): boolean {
    const ui = this.host.ui;
    if (kind === "exit") {
      if (ui.exit.state().phase !== "pending") { this.host.notice("No exit choice is waiting", "text"); return false; }
      this.show({ kind: "exit" }, false);
      return true;
    }
    if (kind === "question") {
      const child = id === undefined ? this.questions()[0] : this.questions().find(entry => entry.id === id);
      if (!child?.question) { this.host.notice(id === undefined ? "No agent is waiting for an answer" : `${id} has no open question`, "text"); return false; }
      this.show({ kind: "question", id: child.id, askedAt: child.question.askedAt }, false);
      return true;
    }
    const proposed = ui.board?.proposed();
    if (!proposed) { this.host.notice("No plan is waiting", "text"); return false; }
    if (proposed !== this.tracked) this.track(proposed);
    this.show({ kind: "plan" }, false);
    return true;
  }

  /** Asking children, oldest `askedAt` first. */
  questions(fleet: FleetSnapshot = this.host.ui.fleet.snapshot()): readonly ChildSnapshot[] {
    return fleet.children.filter(child => child.state === "needs" && child.question)
      .sort((a, b) => a.question!.askedAt - b.question!.askedAt);
  }

  target(): ComposerTarget | undefined {
    if (this.reasonFor !== undefined) return REASON_TARGET;
    const target = this.host.ui.target();
    return target === "parent" ? undefined : { id: target, answer: this.answerFor === target };
  }

  /** For `composerTitle`: shown while any child is steerable, the target is a child, or Enter gives the reason to land. */
  composerTarget(): { readonly label: string; readonly color: string } | undefined {
    if (this.reasonFor !== undefined) return { label: "land · your reason", color: PANEL_WARNING };
    const ui = this.host.ui, fleet = ui.fleet.snapshot(), target = ui.target();
    if (target === "parent") return fleet.children.some(steerable) ? { label: "parent", color: PANEL_BLUE } : undefined;
    return { label: this.answerFor === target ? `${target} · answer` : target, color: this.host.color(childHue(fleet, target)) };
  }

  /** Alt+T: parent, then each steerable child in fleet order, then the parent again. */
  cycleTarget(): void {
    const ui = this.host.ui;
    const order = ["parent", ...ui.fleet.snapshot().children.filter(steerable).map(child => child.id)];
    if (order.length === 1) {
      this.host.notice("No agent is running to receive a message", "text");
      return;
    }
    const next = order[(order.indexOf(ui.target()) + 1) % order.length]!;
    this.setTarget(next);
    this.answerFor = undefined;
    this.reasonFor = undefined;
    this.dock.focused = false;
    this.host.focusComposer();
    this.host.changed();
  }

  /** Points the composer at a steerable child (`s`, opening its conversation, "Write my own answer"); `answer` needs an open question. */
  point(id: string, answer = false): boolean {
    const child = this.host.ui.fleet.snapshot().children.find(entry => entry.id === id);
    if (!child || !steerable(child) || (answer && child.state !== "needs")) return false;
    this.setTarget(id);
    if (this.host.ui.target() !== id) return false;
    this.answerFor = answer ? id : undefined;
    this.reasonFor = undefined;
    this.dock.focused = false;
    this.host.focusComposer();
    this.host.changed();
    return true;
  }

  /** F9: back to the parent. */
  reset(): void {
    this.setTarget("parent");
    this.answerFor = undefined;
    this.reasonFor = undefined;
    this.host.changed();
  }

  /** true when it took the text (child target, answer mode or the reason to land); false: Pi sends it to the parent. Never takes `/` commands. */
  async submit(text: string): Promise<boolean> {
    if (text.trimStart().startsWith("/")) return false;
    if (this.reasonFor !== undefined) {
      const reason = text.trim(), waiting = this.reasonFor, option = this.host.ui.exit.options().find(entry => entry.choice === "land");
      if (!reason) { this.host.notice("Landing needs your reason first", "warning"); return true; }
      if (waiting.episode !== this.episode || this.host.ui.exit.state().phase !== "pending" || !option || option.unavailable !== undefined) {
        this.reasonFor = undefined;
        this.host.notice("The exit choice is no longer waiting · your reason is still in the composer", "warning");
        this.host.changed();
        return true;
      }
      // The reason is for the candidate the owner picked: one prepared since then is theirs to look at first.
      if (option.authorization !== waiting.option.authorization) {
        this.reasonFor = undefined;
        this.host.notice("The Land candidate changed while you typed · your reason is still in the composer; choose Land again", "warning");
        this.show({ kind: "exit" }, false);
        return true;
      }
      this.reasonFor = undefined;
      this.clearDraft(text);
      this.choose(waiting.option, reason, waiting.card);
      return true;
    }
    const target = this.target();
    if (!target || !text.trim()) return false;
    const ui = this.host.ui, id = target.id;
    if (target.answer) {
      const askedAt = ui.fleet.snapshot().children.find(child => child.id === id)?.question?.askedAt;
      this.answerFor = undefined;
      const delivered = await this.answerChild(id, askedAt, text);
      if (this.disposed) return true;
      if (delivered) {
        this.clearDraft(text);
        this.host.notice(`Answered ${id}`, "success");
      } else this.host.notice(`${id} no longer waits for an answer`, "warning");
      this.host.changed();
      return true;
    }
    if (this.steering) return true;
    this.steering = true;
    let delivered = false;
    try { delivered = await ui.fleet.steer(id, text); } catch { delivered = false; } finally { this.steering = false; }
    if (this.disposed) return true;
    if (delivered) {
      this.clearDraft(text);
      this.host.notice(`Steered ${id}`, "success");
      this.mark("steered", id, text);
    } else this.host.notice(`${id} is not running · your message is still in the composer`, "warning");
    this.host.changed();
    return true;
  }

  /**
   * Why F2 Finish cannot run now, or undefined when it can: the same refusals as the app's `exit_open`. An empty
   * branch is no refusal: Keep and Iterate stay the owner's to choose, and the summary known here is the previous
   * episode's, so refusing on it would let the first Finish open and every Finish after an Iterate fail.
   */
  finishUnavailable(): string | undefined {
    const ui = this.host.ui, exit = ui.exit.state(), phase = exit.phase;
    if (phase === "pending") return undefined;
    // A rebase or gate while the candidate is prepared lands nothing: Land only fast-forwards a ready candidate.
    if (exit.candidate?.state === "preparing") return "the Land candidate is still being prepared";
    const done = DONE_REASON[phase];
    if (done) return done;
    if (ui.fleet.snapshot().children.some(child => LIVE_CHILD_STATES.includes(child.state))) return "agents are still running";
    if (this.host.streaming()) return "the parent is still working";
    return undefined;
  }

  /** F2 Finish: opens the exit choice, posts the result card as a conversation entry and docks the dialog. */
  openFinish(): boolean {
    const reason = this.finishUnavailable();
    if (reason) { this.host.notice(`Finish is unavailable: ${reason}`, "warning"); return false; }
    // The host's exit control opens episodes; `BuildUi` types it as the plain `ExitControl`.
    const exit = this.host.ui.exit as ExitControl & { open?: () => void };
    // A host that prepares candidates prepares (or reuses) one for an explicit Finish, then opens the choice.
    if (exit.requestFinish) {
      if (this.finishing) return true;
      const waiting = exit.state().phase === "pending";
      // Docks once, as soon as the choice is pending: at once when the host opens it before its first await.
      let docked = false;
      const dock = (): boolean => {
        if (docked || this.disposed || exit.state().phase !== "pending") return docked;
        docked = true;
        if (!waiting) {
          const entry = finishEntry(`finish-${this.host.now()}-${++this.finishes}`);
          this.latest = entry.details.id;
          this.host.post(entry);
          this.sync();
        }
        this.show({ kind: "exit" }, false);
        return true;
      };
      this.finishing = true;
      const requested = exit.requestFinish();
      dock();
      requested.then(() => { if (!this.disposed && !dock()) this.host.notice("The exit choice could not open", "warning"); },
        error => { if (!this.disposed) this.host.notice(message(error), "error"); })
        .finally(() => { this.finishing = false; if (!this.disposed) this.host.changed(); });
      this.host.changed();
      return true;
    }
    if (exit.state().phase !== "pending") {
      exit.open?.();
      if (exit.state().phase !== "pending") { this.host.notice("The exit choice could not open", "warning"); return false; }
      const entry = finishEntry(`finish-${this.host.now()}-${++this.finishes}`);
      this.latest = entry.details.id;
      this.host.post(entry);
      this.sync();
    }
    this.show({ kind: "exit" }, false);
    return true;
  }

  /** `n` on Board: a draft asking the parent for a board plan, after a blank line when the composer has text; not sent. */
  planDraft(): void {
    const draft = this.host.draft();
    this.host.setDraft(draft.trim() ? `${draft.trimEnd()}\n\n${PLAN_DRAFT}` : PLAN_DRAFT);
    this.dock.focused = false;
    this.host.focusComposer();
    this.host.changed();
  }

  /** Records the exit card that shows live progress: a `build_finish` tool call id. */
  noteFinish(id: string): void {
    this.latest = id;
    for (const watcher of [...this.watchers]) watcher.invalidate();
  }

  dispose(): void {
    this.disposed = true;
    this.watchers.clear();
    this.watching?.dispose();
    this.watching = undefined;
    this.shown = undefined;
    this.dock.focused = false;
  }

  // ---------- BuildCardSource ----------

  state(): BuildPanelState | undefined { return this.host.state(); }
  summary(): BuildSummary | undefined { return this.summaryValue; }
  latestFinish(): string | undefined { return this.latest; }
  color(hue: number): string { return this.host.color(hue); }
  now(): number { return this.host.now(); }
  exitSince(): number | undefined { return this.phaseAt; }
  proposed(): BoardPlan | undefined { return this.host.ui.board?.proposed(); }
  watch(_id: string, invalidate: () => void): () => void {
    const watcher = { invalidate };
    this.watchers.add(watcher);
    return () => { this.watchers.delete(watcher); };
  }
  question(id: string): void { this.open("question", id); }
  exit(): void { this.open("exit"); }
  plan(): void { this.open("plan"); }
  openChild(id: string): void { this.host.openChild(id); }

  // ---------- dialog actions ----------

  private pick(index: number): void {
    const shown = this.shown, dialog = this.dialog(), option = dialog?.options[index];
    if (!shown || !option) return;
    if (option.unavailable !== undefined) { this.refuse(option); return; }
    const ui = this.host.ui;
    if (shown.kind === "exit") {
      const options = ui.exit.options(), choice = options[index];
      if (this.choosing || !choice || choice.unavailable !== undefined) return;
      // The host prepared another candidate since this dialog docked: its card is a new one, and it is the owner's to see first.
      if (options.find(option => option.choice === "land")?.authorization !== this.exitTicket) {
        this.show({ kind: "exit" }, false);
        this.host.notice("The Land candidate changed · check it and choose again", "warning");
        return;
      }
      const card = this.exitCard;
      this.close();
      if (choice.needsReason === undefined) { this.choose(choice, undefined, card); return; }
      // Landing over open findings or without an approval takes the owner's reason, typed in the composer.
      this.reasonFor = { episode: this.episode, option: choice, card };
      this.dock.focused = false;
      this.host.focusComposer();
      const exit = ui.exit.state().candidate, move = exit?.state === "ready" ? fastForward(exit) : undefined;
      this.host.notice(`${choice.needsReason} · type your reason to land anyway; Enter lands${move ? ` (fast-forward ${move})` : ""}`, "warning");
      this.host.changed();
      return;
    }
    if (shown.kind === "question") {
      const child = ui.fleet.snapshot().children.find(entry => entry.id === shown.id), answers = child?.question?.options ?? [];
      const answer = answers[index];
      this.close();
      if (answer) {
        this.answered.add(questionKey(shown.id, shown.askedAt));
        void this.answerChild(shown.id, shown.askedAt, answer.label, index).then(delivered => {
          if (this.disposed) return;
          if (delivered) this.host.notice(`Answered ${shown.id}`, "success");
          else this.host.notice(`${shown.id} no longer waits for an answer`, "warning");
          this.next();
        });
      } else if (!this.point(shown.id, true)) this.host.notice(`${shown.id} no longer waits for an answer`, "warning");
      return;
    }
    // Only a session with a board (not bare) ever tracks a plan.
    const plan = this.tracked, board = ui.board;
    if (!plan || !board || this.filing) return;
    const dismiss = index === 2, start = index === 0;
    if (!dismiss && this.preview.state !== "ready") return;
    if (this.host.dialogs) {
      this.filing = true;
      this.fileError = undefined;
      // The host files (or discards) the plan, and posts the filed tasks to the conversation.
      this.decide({ kind: "plan" }, [{ options: [dismiss ? "dismiss" : start ? "file_start" : "file"] }]).then(status => {
        if (this.disposed) return;
        if (this.shown?.kind === "plan") this.close();
        if (status === "dismissed") this.host.notice("Nothing filed. The plan is discarded.", "text");
        else if (status === "started") this.host.showBoard();
        else this.host.notice("The plan is no longer waiting", "warning");
      }, error => { this.fileError = message(error); })
        .finally(() => { this.filing = false; if (!this.disposed) this.host.changed(); });
      return;
    }
    if (dismiss) {
      board.dismiss();
      this.close();
      this.host.notice("Nothing filed. The plan is discarded.", "text");
      return;
    }
    this.filing = true;
    this.fileError = undefined;
    board.file(plan, start).then(() => {
      if (this.disposed) return;
      if (this.shown?.kind === "plan") this.close();
      this.host.showBoard();
    }, error => { this.fileError = message(error); })
      .finally(() => { this.filing = false; if (!this.disposed) this.host.changed(); });
  }

  private refuse(option: ChoiceOption): void {
    this.host.notice(`${option.label} is unavailable: ${option.unavailable ?? ""}`, "warning");
  }

  /** Esc: not now. The exit stays pending, the child keeps waiting, the plan stays proposed, in the app as well. */
  private dismiss(): void {
    const shown = this.shown;
    if (!shown) return;
    this.close();
    if (shown.kind === "exit") this.host.notice("No exit chosen. Nothing was landed.", "text");
    else if (shown.kind === "question") {
      this.dismissed.add(questionKey(shown.id, shown.askedAt));
      this.host.notice(`${shown.id} keeps waiting`, "text");
    } else this.host.notice("Nothing filed. The plan still waits on the Board tab.", "text");
  }

  // ---------- internals ----------

  /**
   * Docks `shown` and watches its card in the registry; one that opened by itself ignores Enter and digits for a
   * moment and takes focus only then.
   */
  private show(shown: Shown, auto: boolean): void {
    this.shown = shown;
    this.watching?.dispose();
    this.watching = this.host.dialogs?.watch(cardOf(shown), result => this.resolved(shown, result));
    if (shown.kind === "exit") {
      this.reasonFor = undefined;
      // What the owner is shown is what a pick answers: this card and this Land ticket.
      this.exitCard = this.watching?.id;
      this.exitTicket = this.host.ui.exit.options().find(option => option.choice === "land")?.authorization;
    }
    if (shown.kind === "question") this.dismissed.delete(questionKey(shown.id, shown.askedAt));
    this.dock.guard(auto ? Date.now() + CHOICE_GUARD_MS : 0);
    this.dock.focused = true;
    this.host.focusDock();
    this.host.changed();
  }

  /**
   * Runs exit `option` as the owner picked it: through exit card `card` when the registry serves it, else with the
   * option's own Land ticket. `overrideReason` is the owner's reason to land over open findings.
   */
  private choose(option: ExitOption, overrideReason?: string, card?: string): void {
    if (this.choosing) return;
    this.choosing = true;
    const given = {
      ...(overrideReason === undefined ? {} : { overrideReason }),
      ...(option.authorization === undefined ? {} : { authorization: option.authorization }),
    };
    const chosen: Promise<unknown> = this.host.dialogs
      ? this.decide({ kind: "exit" }, [{ options: [option.choice], ...(overrideReason === undefined ? {} : { text: overrideReason }) }], card)
        .then(status => { if (status !== "started" && !this.disposed) this.host.notice("The exit choice is no longer waiting", "warning"); })
      : this.host.ui.exit.choose(option.choice, Object.keys(given).length ? given : undefined);
    chosen.catch(error => { if (!this.disposed) this.host.notice(message(error), "error"); })
      .finally(() => { this.choosing = false; if (!this.disposed) this.host.changed(); });
  }

  /**
   * Answers the registry's card for `shown` from these keys, as the app would; "missing" when no such card waits.
   * The card delivers the decision and the host records its outcome, so nothing here applies or posts it again.
   * `pinned` names the exit card the owner was shown: another exit card is not answered in its place.
   */
  private async decide(shown: Shown, answers: readonly CardAnswer[], pinned?: string): Promise<AnswerStatus> {
    const card = this.host.dialogs?.watch(cardOf(shown, pinned), () => {});
    if (!card) return "missing";
    try { return await card.answer(answers); } finally { card.dispose(); }
  }

  /**
   * Answers child `id`'s question asked at `askedAt` with `text`, an offered option's label when `option` is its
   * index; true once delivered. Through the registry the question's card delivers and records the answer.
   */
  private async answerChild(id: string, askedAt: number | undefined, text: string, option?: number): Promise<boolean> {
    if (!this.host.dialogs) {
      if (!this.host.ui.fleet.answer(id, text)) return false;
      this.mark("answered", id, text);
      return true;
    }
    if (askedAt === undefined) return false;
    const answer: CardAnswer = option === undefined ? { options: [], text } : { options: [optionId(text, option)] };
    try { return await this.decide({ kind: "question", id, askedAt }, [answer]) === "delivered"; } catch { return false; }
  }

  /**
   * The docked decision settled in the registry: by these keys a moment ago, from the app, or as the session closed.
   * The dock closes; an answer from another device says so in one quiet notice. A question the app only put aside
   * (its "not now") is still waiting: the dock stays, still watched, and its keys still answer.
   */
  private resolved(shown: Shown, result: DialogResolution): void {
    if (this.disposed || this.shown !== shown || result.kind === "put_aside") return;
    this.watching = undefined;
    this.close();
    if (result.by?.source !== "remote") return;
    this.host.notice(result.kind === "answered" ? "Answered from another device" : "Dismissed from another device", "text");
    if (shown.kind === "question") {
      this.answered.add(questionKey(shown.id, shown.askedAt));
      this.next();
    }
  }

  private close(): void {
    this.shown = undefined;
    this.watching?.dispose();
    this.watching = undefined;
    if (this.dock.focused) {
      this.dock.focused = false;
      this.host.focusComposer();
    }
    this.host.changed();
  }

  /** After an answer, the oldest question still waiting docks, under the same rule as a new one. */
  private next(fleet?: FleetSnapshot): void {
    if (this.shown || this.host.overlay() || this.host.draft().trim()) return;
    const child = this.questions(fleet).find(entry => {
      const key = questionKey(entry.id, entry.question!.askedAt);
      return !this.dismissed.has(key) && !this.answered.has(key);
    });
    if (child) this.show({ kind: "question", id: child.id, askedAt: child.question!.askedAt }, true);
  }

  /** A new proposed plan starts a plan episode and its preview. */
  private track(plan: BoardPlan | undefined): void {
    this.tracked = plan;
    this.fileError = undefined;
    const board = this.host.ui.board;
    if (!plan || !board) return;
    this.planEpisode++;
    this.preview = { state: "loading" };
    board.preview(plan).then(
      value => { if (this.tracked === plan) this.preview = { state: "ready", value }; },
      error => { if (this.tracked === plan) this.preview = { state: "failed", error: message(error) }; },
    ).finally(() => { if (!this.disposed && this.tracked === plan) this.host.changed(); });
  }

  private readSummary(episode: number): void {
    this.host.ui.exit.summary().then(value => {
      if (this.disposed || episode !== this.episode) return;
      this.summaryValue = value;
      for (const watcher of [...this.watchers]) watcher.invalidate();
      this.host.changed();
    }, () => {});
  }

  private setTarget(target: string): void {
    this.host.ui.setTarget(target);
    this.lastTarget = this.host.ui.target();
  }

  /** Clears the composer only when it still holds what was sent, so typing during delivery is kept. */
  private clearDraft(text: string): void {
    if (this.host.draft() === text) this.host.setDraft("");
  }

  /** Records what passed between the owner and child `child` in the transcript. */
  private mark(act: ChildMark["act"], child: string, text: string, at = this.host.now()): void {
    this.host.marks?.add({ kind: "child", at, act, child, text });
  }
}
