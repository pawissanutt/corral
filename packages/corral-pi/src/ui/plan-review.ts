// The terminal's plan review: the submitted plan in the pager, with the owner's choices as its keys. The app
// answers the same submission through its card; whichever answers first closes the other.
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { PlanChoice, PlanMode } from "../host/plan-mode.ts";
import { openPager, type PagerAction, type PagerLine } from "./pager.ts";

const CHOICES: Readonly<Record<string, PlanChoice>> = { e: "execute", c: "execute_clean", s: "execute_compact" };
const ACTIONS: readonly PagerAction[] = [
  { key: "e", label: "execute" }, { key: "c", label: "clean context" }, { key: "s", label: "compacted" },
  { key: "m", label: "model" }, { key: "k", label: "keep planning" },
];

export interface PlanReviewOptions {
  model(): string | undefined;
  /** Pi's own model selector; absent where this host has none to open. */
  selectModel?(): boolean;
  notice(text: string): void;
}

export class PlanReviewUi {
  private ui: ExtensionUIContext | undefined;
  private shown: AbortController | undefined;
  /** The submission the review opened for unasked, so each is offered once. */
  private offered: string | undefined;
  /** The submission to return to once a model selection settles. */
  private selecting: string | undefined;

  constructor(private readonly plan: PlanMode, private readonly options: PlanReviewOptions) {}

  install(pi: ExtensionAPI): void {
    pi.on("session_start", (_event, ctx) => { if (ctx.mode === "tui") this.ui = ctx.ui; });
    // The agent submits inside its turn; the review opens once that turn has ended.
    pi.on("agent_settled", () => {
      const id = this.plan.review;
      if (id === undefined || id === this.offered) return;
      this.offered = id;
      void this.show();
    });
    pi.on("model_select", () => {
      const id = this.selecting;
      this.selecting = undefined;
      // Pi closes its selector and hands the composer the keys only after this handler has returned. A review
      // opened here would be drawn without them, so it reopens in a later task.
      if (id !== undefined) setTimeout(() => { if (id === this.plan.review) void this.show(); }, 0);
    });
    const unsubscribe = this.plan.onChange(() => { if (this.plan.review === undefined) this.shown?.abort(); });
    pi.on("session_shutdown", () => {
      unsubscribe();
      this.shown?.abort();
    });
  }

  /** Open the review of the pending submission; false when there is none or no terminal to draw it in. */
  async show(): Promise<boolean> {
    const ui = this.ui, id = this.plan.review;
    if (!ui || id === undefined) return false;
    if (this.shown) return true;
    const { title, revision } = this.plan.record;
    const text = this.plan.reviewText();
    const lines: (string | PagerLine)[] = text === undefined
      ? ["The submitted plan's copy is missing or was changed. Choose keep planning and have it submitted again."]
      : text.split("\n").map(line => /^#{1,6}\s/.test(line) ? { text: line, heading: true } : line);
    const control = this.shown = new AbortController();
    let key: string | undefined;
    try {
      key = await openPager(ui, { title: `Plan review · ${title ?? "Plan"} · revision ${revision} · ${this.options.model() ?? "no model"}`,
        lines, wrap: true, actions: ACTIONS }, control.signal);
    } finally {
      if (this.shown === control) this.shown = undefined;
    }
    if (key === undefined || this.plan.review !== id) return true;
    if (key === "m") {
      if (this.options.selectModel?.()) this.selecting = id;
      else this.options.notice("Change the model with /model, then reopen the review with /plan.");
      return true;
    }
    let feedback: string | undefined;
    if (key === "k") {
      feedback = await ui.input("What should change in the plan?", "Feedback for the agent; leave empty to type it as a message");
      // Esc on the feedback returns to the review rather than deciding anything.
      if (feedback === undefined) return this.show();
    }
    const choice = key === "k" ? "revise" : CHOICES[key];
    if (!choice) return true;
    // Typed feedback that was not delivered stays the owner's: it joins whatever the composer already holds.
    const keep = (why: string) => {
      const typed = feedback?.trim();
      if (typed) {
        const draft = ui.getEditorText();
        ui.setEditorText(draft.trim() ? `${draft}\n\n${typed}` : typed);
      }
      this.options.notice(`${why}${typed ? ". Your feedback is in the composer." : ""}`);
    };
    if (this.plan.review !== id) {
      // The review was answered elsewhere while the feedback was being typed.
      if (feedback?.trim()) keep("This review was answered from another device");
      return true;
    }
    // A choice the session cannot carry out now leaves the review pending; /plan reopens it.
    try { await this.plan.choose(choice, feedback); }
    catch (error) { keep(error instanceof Error ? error.message : String(error)); }
    return true;
  }
}
