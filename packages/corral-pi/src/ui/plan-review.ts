// The terminal's plan review: the submitted plan in its reading view (plan-view.ts), which returns the owner's
// choice. An execute choice starts the plan on the model the review named. Keep planning decides nothing here: the
// owner's next message is the feedback, as any message of theirs is while a review is open, so the headings they
// marked go to the composer and the review stays open until that message is sent. The app answers the same
// submission through its card; whichever answers first closes the other.
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { PlanMode } from "../host/plan-mode.ts";
import { openPlanView, type PlanViewResult } from "./plan-view.ts";

export interface PlanReviewOptions {
  model(): string | undefined;
  /** Pi's own model selector; absent where this host has none to open. */
  selectModel?(): boolean;
  notice(text: string): void;
}

export class PlanReviewUi {
  private ui: ExtensionUIContext | undefined;
  /** The open review, the submission it shows and the model it names as the one that will execute. */
  private shown: { readonly id: string; readonly model: string | undefined; readonly control: AbortController } | undefined;
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
      // Changed from elsewhere, as from the app, under an open review: it closes and is shown again with the
      // model an execute key would now run on.
      else setTimeout(() => { if (this.shown && this.options.model() !== this.shown.model) this.shown.control.abort(); }, 0);
    });
    // The review on screen is gone, or a rewind put another in its place: it must not go on showing that plan.
    const unsubscribe = this.plan.onChange(() => { if (this.plan.review !== this.shown?.id) this.shown?.control.abort(); });
    pi.on("session_shutdown", () => {
      unsubscribe();
      // Nothing is shown again once the terminal is gone.
      this.ui = undefined;
      this.shown?.control.abort();
    });
  }

  /** Read the approved document without reopening its choices. */
  async showApproved(): Promise<void> {
    const approved = this.plan.approved;
    if (this.ui && approved) await openPlanView(this.ui, { ...approved, model: undefined, text: this.plan.approvedText(), readonly: true });
  }

  /** Open the review of the pending submission; false when there is none or no terminal to draw it in. */
  async show(): Promise<boolean> {
    const ui = this.ui, id = this.plan.review;
    if (!ui || id === undefined) return false;
    if (this.shown) return true;
    const { title, revision } = this.plan.record;
    // The model that will execute the plan is named in the view: an execute choice approves a run on that model.
    const model = this.options.model();
    const control = new AbortController(), open = this.shown = { id, model, control };
    let result: PlanViewResult | undefined;
    try {
      result = await openPlanView(ui, { title: title ?? "Plan", revision, model, text: this.plan.reviewText(), previous: this.plan.previousText() }, control.signal);
    } finally {
      if (this.shown === open) this.shown = undefined;
    }
    const now = this.plan.review;
    // Closed under the owner because another review took its place: show that one instead.
    if (control.signal.aborted && now !== undefined && now !== id) {
      this.offered = now;
      return this.show();
    }
    // An execute key approves a run on the model the review named. If the model was changed from elsewhere, the
    // review closed for it or the key came first: either way it is shown again, and nothing is approved yet.
    const executes = result !== undefined && "choice" in result && result.choice !== "revise";
    if (now === id && this.options.model() !== model && (control.signal.aborted || executes)) return this.show();
    if (result === undefined || now !== id) return true;
    if ("model" in result) {
      if (this.options.selectModel?.()) this.selecting = id;
      else this.options.notice("Change the model with /model, then reopen the review with /plan.");
      return true;
    }
    if (result.choice === "revise") {
      // Each marked heading is quoted on a line of its own, after whatever the composer already holds.
      const marks = result.quotes.map(heading => `> ${heading}\n\n`).join("");
      if (marks) {
        const draft = ui.getEditorText();
        // The draft stays as it was typed; the quotes follow it after one blank line.
        ui.setEditorText(draft + (!draft || draft.endsWith("\n\n") ? "" : draft.endsWith("\n") ? "\n" : "\n\n") + marks);
      }
      this.options.notice("Write what should change and send it: the plan stays under review until you do · /plan reopens it");
      return true;
    }
    // A choice the session cannot carry out now leaves the review pending; /plan reopens it.
    try { await this.plan.choose(result.choice); }
    catch (error) { this.options.notice(error instanceof Error ? error.message : String(error)); }
    return true;
  }
}
