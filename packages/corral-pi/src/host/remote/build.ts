import { BridgeRefusal } from "../../bridge/protocol.ts";
import type { BuildControls } from "../../build/host-controls.ts";
import { LIVE_CHILD_STATES, type BoardPlan, type ChildSnapshot, type ExitOption, type ExitState, type FleetControl } from "../../build/types.ts";
import { boardEntry, exitEntry, finishEntry, type BuildEntry } from "../../ui/build/build-entries.ts";
import { policyLabel } from "../../ui/build/build-dialogs.ts";
import { taskMetadataLabel } from "../../ui/build/task-metadata.ts";
import type { ChildMark } from "../transcript-marks.ts";
import { cardOptions, cardQuestion, invalidAnswer, optionId, singleAnswer, type CardEntry, type CardOption } from "./cards.ts";
import type { RemoteRegistry } from "./registry.ts";

interface Episode { id: string; opened_at: string }
interface ExitEpisode extends Episode { readonly authorization?: string }
function episode(kind: string, at = Date.now()): Episode {
  return { id: `${kind}-${crypto.randomUUID()}`, opened_at: new Date(at).toISOString() };
}
const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);
const closedPhases: readonly ExitState["phase"][] = ["rebasing", "verifying", "repairing", "landing"];
const BOARD_OPTIONS: CardOption[] = [
  { id: "file_start", label: "File and start", detail: "File the plan and queue its tasks.", destructive: false },
  { id: "file", label: "File only", detail: "File the plan without starting tasks.", destructive: false },
  { id: "dismiss", label: "Dismiss", detail: "Discard the proposal without filing it.", destructive: false },
];

export class RemoteBuild {
  private readonly children = new Map<string, Episode & { askedAt: number }>();
  private exitEpisode: ExitEpisode | undefined;
  private exitOpening = false;
  private exitReading = false;
  private exitChoosing = false;
  private exitOutcome: ((state: ExitState) => void) | undefined;
  private exitFailure: string | undefined;
  private planEpisode: (Episode & { plan: BoardPlan; error?: string }) | undefined;
  private readonly exitPolicy: string | undefined;
  private filing = false;

  constructor(private readonly build: Pick<BuildControls, "exit" | "board" | "panelState"> | undefined, private readonly host: {
    fleet(): Pick<FleetControl, "answer"> | undefined;
    children(): readonly ChildSnapshot[];
    ready(): boolean;
    headless(): boolean;
    working(): boolean;
    changed(): void;
    mark(mark: ChildMark): void;
    post(entry: BuildEntry<unknown>): Promise<void>;
    error(text: string): void;
  }) {
    build?.exit.subscribe(() => this.observeExit());
    this.exitPolicy = build ? policyLabel(build.panelState()) : undefined;
    if (build?.exit.state().phase === "pending") this.exitEpisode = episode("exit");
  }

  register(registry: RemoteRegistry): void {
    if (!this.build) return;
    registry.register({ name: "exit_open", parse: () => undefined, run: () => ({ phase: this.openExit() }) });
  }

  private observeExit(): void {
    const state = this.build?.exit.state(), phase = state?.phase;
    if (state && (phase === "none" || phase === "kept" || phase === "landed" || phase === "failed")) {
      this.exitOutcome?.(state);
    }
    if (phase !== "pending") {
      this.exitEpisode = undefined;
      this.exitReading = false;
    } else if (!this.exitEpisode) {
      this.exitEpisode = episode("exit");
      this.exitReading = this.build?.exit.summarySnapshot() === undefined;
      this.exitFailure = undefined;
    } else this.exitReading = false;
    this.host.changed();
  }

  exitState(): ExitState | undefined {
    const state = this.build?.exit.state();
    return this.exitFailure === undefined ? state : { phase: "failed", detail: this.exitFailure, record: state?.record };
  }

  refreshExit(): void {
    const exit = this.build?.exit;
    if (!exit || !this.host.ready() || !this.exitEpisode || this.exitChoosing || this.exitReading || exit.busy()) return;
    this.exitReading = true;
    this.host.changed();
    void exit.summary().catch(error => this.host.error(errorMessage(error))).finally(() => {
      this.exitReading = false;
      this.host.changed();
    });
  }

  private openExit(): ExitState["phase"] {
    const exit = this.build?.exit;
    if (!exit || !this.host.ready()) throw new BridgeRefusal("host_not_ready", "session cannot open an exit");
    const phase = exit.state().phase;
    if (this.exitOpening || this.exitChoosing || exit.busy() || closedPhases.includes(phase)) return phase;
    if (this.host.working() || this.host.children().some(child => LIVE_CHILD_STATES.includes(child.state))) {
      if (phase === "pending") return phase;
      throw new BridgeRefusal("exit_unavailable", "wait for the parent and its agents to finish");
    }
    const previous = this.exitEpisode;
    this.exitFailure = undefined;
    this.exitOpening = true;
    // Preparing and collecting evidence can outlive a socket request; publish the card when it settles.
    void Promise.resolve().then(() => exit.requestFinish()).then(() => {
      const opened = this.exitCard()?.card;
      if (opened && opened.id !== previous?.id && this.host.headless()) return this.host.post(finishEntry(opened.id));
    }).catch(error => {
      this.exitFailure = errorMessage(error);
      this.host.error(this.exitFailure);
    }).finally(() => { this.exitOpening = false; this.host.changed(); });
    return phase;
  }

  private chooseExit(card: ExitEpisode, choice: string, reason: string | undefined): "started" | "missing" {
    const exit = this.build?.exit;
    if (!exit || card !== this.exitEpisode || this.exitChoosing || exit.busy() || this.exitFailure !== undefined) return "missing";
    const check = (option: ExitOption | undefined): ExitOption => {
      if (!option) invalidAnswer("choose a listed exit option");
      if (option.needsReason && !reason?.trim()) throw new BridgeRefusal("exit_reason_required", option.needsReason);
      if (option.unavailable) throw new BridgeRefusal("exit_unavailable", option.unavailable);
      return option;
    };
    const option = check(exit.options().find(option => option.choice === choice));
    if (option.choice === "land" && (!card.authorization || option.authorization !== card.authorization)) return "missing";
    if (option.choice === "land" && (this.exitReading || this.host.working())) {
      throw new BridgeRefusal("exit_unavailable", "wait for the branch summary and parent turn to finish");
    }
    this.exitChoosing = true;
    this.host.changed();
    const release = () => { this.exitOutcome = undefined; this.exitChoosing = false; this.host.changed(); };
    try {
      let posted = false;
      const post = (state: ExitState) => {
        if (posted) return;
        posted = true;
        void this.host.post(exitEntry(option, state)).catch(error => this.host.error(errorMessage(error)));
      };
      this.exitOutcome = post;
      void exit.choose(option.choice, option.choice === "land" ? { authorization: card.authorization, overrideReason: reason } : undefined).then(post, error => {
        this.exitFailure = errorMessage(error);
        this.host.changed();
        post({ phase: "failed", detail: this.exitFailure });
      }).finally(release);
      return "started";
    } catch (error) { release(); throw error; }
  }

  private exitCard(): CardEntry | undefined {
    let pending = this.exitEpisode;
    const exit = this.build?.exit;
    if (!pending || !exit || this.exitChoosing || exit.busy() || this.exitFailure !== undefined) return;
    const choices = exit.options();
    const authorization = choices.find(option => option.choice === "land")?.authorization;
    // A remote answer names its card, not a SHA. A new offer therefore MUST have a new card ID.
    if (pending.authorization !== authorization) pending = this.exitEpisode = { ...episode("exit"), authorization };
    const title = "How should this leave the build session?";
    const summary = exit.summarySnapshot();
    const body = summary ? [
      `${summary.commits} commits · ${summary.files} files · +${summary.added} −${summary.removed}`,
      ...(summary.dirty ? ["! Uncommitted changes"] : []),
      ...(summary.outside.length ? [`! ${summary.outside.length} files changed outside child ownership`] : []),
    ].join("\n") : undefined;
    const options = choices.map(option => ({
      id: option.choice, label: option.label, detail: option.detail, needs_text: option.needsReason,
      unavailable: option.unavailable ?? (option.choice === "land" && this.exitReading ? "The branch summary is still being read."
        : option.choice === "land" && this.host.working() ? "Wait for the parent turn to finish." : undefined),
      destructive: option.choice === "land",
    }));
    return { card: { ...pending, kind: "exit", title, meta: this.exitPolicy, body, questions: [cardQuestion(title, options)] }, answer: (answers, dismiss) => {
      if (dismiss) return this.chooseExit(pending, "iterate", undefined);
      const { options, text } = singleAnswer(answers);
      const choice = options[0];
      if (choice === undefined || choice !== "land" && text !== undefined) invalidAnswer("choose an exit option");
      return this.chooseExit(pending, choice, text);
    } };
  }

  private boardCard(): CardEntry | undefined {
    const board = this.build?.board, plan = board?.proposed();
    if (!board || !plan) { this.planEpisode = undefined; return; }
    if (this.planEpisode?.plan !== plan) this.planEpisode = { ...episode("board-plan"), plan };
    const pending = this.planEpisode;
    if (this.filing) return;
    const title = plan.title;
    // Keep metadata ahead of potentially long titles in the bounded card body. The board part carries structured tags.
    const metadata = plan.tasks.flatMap(task => {
      const label = taskMetadataLabel(task);
      return label ? [`${JSON.stringify(task.key)}: ${label}`] : [];
    });
    const tasks = plan.tasks.map(task => `${task.key}: ${task.title}${task.depends.length ? ` (after ${task.depends.join(", ")})` : ""}`);
    const body = [pending.error, metadata.length && `Task metadata\n${metadata.join("\n")}`, tasks.join("\n"), plan.why,
      ...plan.tasks.map(task => task.body && `${task.key}\n${task.body}`)]
      .filter(Boolean).join("\n\n");
    const meta = [plan.project, `${plan.tasks.length} tasks`].filter(Boolean).join(" · ");
    return { card: { id: pending.id, opened_at: pending.opened_at, kind: "board_plan", title, meta, body, tool: "board_plan",
      questions: [cardQuestion("File this plan on the board?", BOARD_OPTIONS)] }, answer: (answers, dismiss) => {
      if (this.filing || board.proposed() !== plan) return "missing";
      const answer = dismiss ? { options: ["dismiss"] } : singleAnswer(answers);
      const choice = answer.options[0];
      if (answer.text !== undefined || !BOARD_OPTIONS.some(option => option.id === choice)) invalidAnswer("choose a board plan action");
      if (choice === "dismiss") { board.dismiss(); return "dismissed"; }
      const start = choice === "file_start", before = new Set(board.tasks().map(task => task.id));
      this.filing = true;
      this.host.changed();
      void board.file(plan, start).then(tasks => this.host.post(boardEntry(tasks.filter(task => !before.has(task.id)), start))).catch(error => {
        pending.error = errorMessage(error);
        this.host.error(pending.error);
      }).finally(() => { this.filing = false; this.host.changed(); });
      return "started";
    } };
  }

  entries(): CardEntry[] {
    const cards: CardEntry[] = [], current = new Set<string>();
    for (const child of this.host.children()) {
      const question = child.question;
      if (!question || child.state !== "needs") continue;
      current.add(child.id);
      let pending = this.children.get(child.id);
      if (!pending || pending.askedAt !== question.askedAt) {
        pending = { ...episode("child-question", question.askedAt), askedAt: question.askedAt };
        this.children.set(child.id, pending);
        if (this.host.headless()) this.host.mark({ kind: "child", child: child.id, act: "asked", at: question.askedAt, text: question.question });
      }
      const id = pending.id;
      cards.push({ card: { id, opened_at: pending.opened_at, kind: "child_question", child: child.id,
        title: question.question, meta: `${child.profile} · ${child.model}`, ...(Buffer.byteLength(question.question) > 160 ? { body: question.question } : {}),
        questions: [cardQuestion(question.question, cardOptions(question.options), true)] },
      answer: (answers, dismiss) => {
        if (dismiss) return "dismissed";
        const { options, text } = singleAnswer(answers);
        const option = options.length ? question.options.find(({ label }, i) => optionId(label, i) === options[0]) : undefined;
        if (options.length ? !option || text !== undefined : !text?.trim()) invalidAnswer("choose a listed option or enter text");
        const response = option?.label ?? text!;
        if (!this.host.fleet()?.answer(child.id, response)) return "missing";
        this.children.delete(child.id);
        this.host.mark({ kind: "child", child: child.id, act: "answered", at: Date.now(), text: response });
        return "delivered";
      } });
    }
    for (const id of this.children.keys()) if (!current.has(id)) this.children.delete(id);
    const exit = this.exitCard(), board = this.boardCard();
    if (exit) cards.push(exit);
    if (board) cards.push(board);
    return cards;
  }
}
