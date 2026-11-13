// What a build session's UI decides on its own: which docked dialog shows and when, who the composer's
// Enter reaches, and which outcomes become conversation entries. Views stay pure; this keeps the episodes.
import type {
  BoardPlan, BuildPanelState, BuildSummary, BuildUi, ChildSnapshot, ExitControl, ExitOption, ExitPhase, FleetSnapshot,
} from "../../build/types.ts";
import { LIVE_CHILD_STATES } from "../../build/types.ts";
import { PANEL_BLUE } from "../panels-view.ts";
import { exitDialog, planDialog, questionDialog } from "./build-dialogs.ts";
import type { PreviewState } from "./build-dialogs.ts";
import { boardEntry, exitEntry, finishEntry } from "./build-entries.ts";
import type { BuildEntry } from "./build-entries.ts";
import { CHOICE_GUARD_MS, ChoiceDock } from "./choice-dock.ts";
import type { ChoiceDialog, ChoiceOption } from "./choice-dock.ts";
import { childHue } from "./contract.ts";
import type { BuildCardSource, ComposerTarget } from "./contract.ts";
import { steerable } from "./fleet-rows.ts";

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
  showBoard(): void;
  openChild(id: string): void;
  color(hue: number): string;
  now(): number;
  changed(): void;
}

type Shown = { readonly kind: "exit" } | { readonly kind: "question"; readonly id: string; readonly askedAt: number } | { readonly kind: "plan" };

const PLAN_DRAFT = "Split the remaining work into board tasks and propose them with board_plan.";
/** Phases after which an exit episode has an outcome the parent should learn. */
const SETTLED: readonly ExitPhase[] = ["kept", "shipped", "landed", "failed"];
const DONE_REASON: Partial<Record<ExitPhase, string>> = {
  kept: "this session already kept the branch", shipped: "this session already shipped",
  landing: "this session is already landing", landed: "this session already landed",
};

const questionKey = (id: string, askedAt: number) => `${id}:${askedAt}`;
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

export class BuildController implements BuildCardSource {
  readonly dock: ChoiceDock;
  private shown: Shown | undefined;
  private lastPhase: ExitPhase;
  private lastTarget: string;
  /** Exit episodes: one per entry into `pending`. */
  private episode = 0;
  private posted = -1;
  private chosen: ExitOption | undefined;
  private choosing = false;
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
  private readonly watchers = new Set<{ readonly invalidate: () => void }>();
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

  /** Every snapshot: track exit/plan/question episodes, auto-open, fetch the summary once per exit episode, start previews, post terminal exit entries, notice a target fallback. */
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
    const exit = ui.exit.state(), phase = exit.phase;
    if (phase !== this.lastPhase) {
      const previous = this.lastPhase;
      this.lastPhase = phase;
      if (phase === "pending") {
        this.episode++;
        this.chosen = undefined;
        this.summaryValue = undefined;
        this.readSummary(this.episode);
        arrivals.push({ kind: "exit" });
      } else {
        if (this.shown?.kind === "exit") this.close();
        const iterated = phase === "none" && this.chosen?.choice === "iterate" && (previous === "pending" || previous === "running");
        if (this.posted !== this.episode && (SETTLED.includes(phase) || iterated)) {
          const option = this.chosen ?? ui.exit.options().find(entry => entry.choice === exit.record?.choice);
          if (option) { this.posted = this.episode; this.host.post(exitEntry(option, exit)); }
        }
      }
    }

    const asking = this.questions(fleet);
    for (const child of asking) {
      const key = questionKey(child.id, child.question!.askedAt);
      if (!this.seen.has(key)) { this.seen.add(key); arrivals.push({ kind: "question", id: child.id, askedAt: child.question!.askedAt }); }
    }
    const shown = this.shown;
    if (shown?.kind === "question" && !asking.some(child => child.id === shown.id && child.question!.askedAt === shown.askedAt)) {
      this.close();
      this.next(fleet);
    }

    const proposed = ui.board.proposed();
    if (proposed !== this.tracked) {
      this.track(proposed);
      if (proposed) arrivals.push({ kind: "plan" });
      else if (this.shown?.kind === "plan") this.close();
    }

    const first = arrivals[0];
    if (first && !this.shown && !this.host.overlay() && !this.host.draft().trim()) this.show(first, true);
    for (const arrival of arrivals) {
      if (arrival.kind === "question" && this.shown !== arrival) this.host.notice(`${arrival.id} asks a question`, "warning");
    }
    for (const watcher of [...this.watchers]) watcher.invalidate();
  }

  /** The docked dialog, or undefined when none is shown or what it asked about is gone. */
  dialog(): ChoiceDialog | undefined {
    const shown = this.shown, ui = this.host.ui;
    if (!shown) return undefined;
    if (shown.kind === "exit") {
      const state = this.host.state();
      if (!state || ui.exit.state().phase !== "pending") return undefined;
      const live = ui.fleet.snapshot().children.filter(child => LIVE_CHILD_STATES.includes(child.state)).length;
      return exitDialog(state, ui.exit.options(), this.episode, live);
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
    const proposed = ui.board.proposed();
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
    const target = this.host.ui.target();
    return target === "parent" ? undefined : { id: target, answer: this.answerFor === target };
  }

  /** For `composerTitle`: shown while any child is steerable or the target is a child. */
  composerTarget(): { readonly label: string; readonly color: string } | undefined {
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
    this.dock.focused = false;
    this.host.focusComposer();
    this.host.changed();
    return true;
  }

  /** F9: back to the parent. */
  reset(): void {
    this.setTarget("parent");
    this.answerFor = undefined;
    this.host.changed();
  }

  /** true when it took the text (child target or answer mode); false: Pi sends it to the parent. Never takes `/` commands. */
  async submit(text: string): Promise<boolean> {
    const target = this.target();
    if (!target || !text.trim() || text.trimStart().startsWith("/")) return false;
    const ui = this.host.ui, id = target.id;
    if (target.answer) {
      if (ui.fleet.answer(id, text)) {
        this.answerFor = undefined;
        this.clearDraft(text);
        this.host.notice(`Answered ${id}`, "success");
      } else {
        this.answerFor = undefined;
        this.host.notice(`${id} no longer waits for an answer`, "warning");
      }
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
    } else this.host.notice(`${id} is not running · your message is still in the composer`, "warning");
    this.host.changed();
    return true;
  }

  /** Why F2 Finish cannot run now, or undefined when it can. */
  finishUnavailable(): string | undefined {
    const ui = this.host.ui, phase = ui.exit.state().phase;
    if (phase === "pending") return undefined;
    if (phase === "running") return "an exit is already running";
    const done = DONE_REASON[phase];
    if (done) return done;
    const children = ui.fleet.snapshot().children;
    if (children.some(child => LIVE_CHILD_STATES.includes(child.state))) return "agents are still running";
    if (this.host.streaming()) return "the parent is still working";
    const summary = this.summaryValue;
    if (!children.length && summary && summary.commits === 0 && !summary.dirty) return "nothing was built yet";
    return undefined;
  }

  /** F2 Finish: opens the exit choice, posts the result card as a conversation entry and docks the dialog. */
  openFinish(): boolean {
    const reason = this.finishUnavailable();
    if (reason) { this.host.notice(`Finish is unavailable: ${reason}`, "warning"); return false; }
    // The host's exit control opens episodes; `BuildUi` types it as the plain `ExitControl`.
    const exit = this.host.ui.exit as ExitControl & { open?: () => void };
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
    this.shown = undefined;
    this.dock.focused = false;
  }

  // ---------- BuildCardSource ----------

  state(): BuildPanelState | undefined { return this.host.state(); }
  summary(): BuildSummary | undefined { return this.summaryValue; }
  latestFinish(): string | undefined { return this.latest; }
  color(hue: number): string { return this.host.color(hue); }
  now(): number { return this.host.now(); }
  proposed(): BoardPlan | undefined { return this.host.ui.board.proposed(); }
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
      const choice = ui.exit.options()[index];
      if (this.choosing || !choice || choice.unavailable !== undefined) return;
      this.choosing = true;
      this.chosen = choice;
      this.close();
      ui.exit.choose(choice.choice)
        .catch(error => { if (!this.disposed) this.host.notice(message(error), "error"); })
        .finally(() => { this.choosing = false; if (!this.disposed) this.host.changed(); });
      return;
    }
    if (shown.kind === "question") {
      const child = ui.fleet.snapshot().children.find(entry => entry.id === shown.id), answers = child?.question?.options ?? [];
      const answer = answers[index];
      this.close();
      if (answer) {
        this.answered.add(questionKey(shown.id, shown.askedAt));
        if (ui.fleet.answer(shown.id, answer.label)) this.host.notice(`Answered ${shown.id}`, "success");
        else this.host.notice(`${shown.id} no longer waits for an answer`, "warning");
        this.next();
      } else if (!this.point(shown.id, true)) this.host.notice(`${shown.id} no longer waits for an answer`, "warning");
      return;
    }
    const plan = this.tracked;
    if (!plan || this.filing) return;
    if (index === 2) {
      ui.board.dismiss();
      this.close();
      this.host.notice("Nothing filed. The plan is discarded.", "text");
      return;
    }
    if (this.preview.state !== "ready") return;
    const start = index === 0, before = new Set(ui.board.tasks().map(task => task.id));
    this.filing = true;
    this.fileError = undefined;
    ui.board.file(plan, start).then(tasks => {
      if (this.disposed) return;
      this.host.post(boardEntry(tasks.filter(task => !before.has(task.id)), start));
      if (this.shown?.kind === "plan") this.close();
      this.host.showBoard();
    }, error => { this.fileError = message(error); })
      .finally(() => { this.filing = false; if (!this.disposed) this.host.changed(); });
  }

  private refuse(option: ChoiceOption): void {
    this.host.notice(`${option.label} is unavailable: ${option.unavailable ?? ""}`, "warning");
  }

  /** Esc: not now. The exit stays pending, the child keeps waiting, the plan stays proposed. */
  private dismiss(): void {
    const shown = this.shown;
    if (!shown) return;
    this.close();
    if (shown.kind === "exit") this.host.notice("No exit chosen. Nothing was filed or landed.", "text");
    else if (shown.kind === "question") {
      this.dismissed.add(questionKey(shown.id, shown.askedAt));
      this.host.notice(`${shown.id} keeps waiting`, "text");
    } else this.host.notice("Nothing filed. The plan still waits on the Board tab.", "text");
  }

  // ---------- internals ----------

  /** Docks `shown`; one that opened by itself ignores Enter and digits for a moment and takes focus only then. */
  private show(shown: Shown, auto: boolean): void {
    this.shown = shown;
    if (shown.kind === "question") this.dismissed.delete(questionKey(shown.id, shown.askedAt));
    this.dock.guard(auto ? Date.now() + CHOICE_GUARD_MS : 0);
    this.dock.focused = true;
    this.host.focusDock();
    this.host.changed();
  }

  private close(): void {
    this.shown = undefined;
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
    if (!plan) return;
    this.planEpisode++;
    this.preview = { state: "loading" };
    this.host.ui.board.preview(plan).then(
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
}
