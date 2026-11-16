// The one goal-aware handoff sheet (T-807 §4/§8). Implementation work offers Save for later /
// Take over and finish; a plan offers Save plan / Continue planning / Review this plan. Focus
// starts on context selection, never on an action; Enter and a click activate only the focused
// or clicked control. Escape before a choice does nothing but close. The sheet only reports the
// stable option and context: the host journals the native Choice and calls the shared service.
import { matchesKey, truncateToWidth, wrapTextWithAnsi, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";

export type ContextMode = "task" | "conversation" | "minimal";
export type SheetAction = { id: string; label: string; stop: string; disabled?: string };
export type SheetResult = { action: string; context: ContextMode } | undefined;

export const CONTEXTS: readonly { id: ContextMode; label: string; description: string }[] = [
  { id: "task", label: "Task context · Recommended", description: "Goal, decisions, current work, findings and next steps." },
  { id: "conversation", label: "Conversation context", description: "Task context plus selected conversation history." },
  { id: "minimal", label: "Minimal context", description: "Task brief and current work, without discussion history." },
];

export function implementationActions(disabled: Partial<Record<string, string>> = {}): SheetAction[] {
  return [
    { id: "save_for_later", label: "Save for later", stop: "No agent starts", disabled: disabled.save_for_later },
    { id: "continue_selected_goal", label: "Take over and finish", stop: "Board continues under its policy", disabled: disabled.continue_selected_goal },
  ];
}

export function planActions(disabled: Partial<Record<string, string>> = {}): SheetAction[] {
  return [
    { id: "save_for_later", label: "Save plan for later", stop: "Held plan; no agent starts", disabled: disabled.save_for_later },
    { id: "plan_continue", label: "Continue planning", stop: "Stops at a reviewed plan; never code", disabled: disabled.plan_continue },
    { id: "plan_review_only", label: "Review this plan", stop: "One independent review; findings only", disabled: disabled.plan_review_only },
  ];
}

/** Rows: contexts first, then actions, then Cancel. `focus` indexes that list. */
export class HandoffSheet implements Component {
  focus = 0;
  context: ContextMode = "task";
  private finished = false;
  private rowAt = new Map<number, number>();
  /** First visible content line when the sheet is taller than its row budget. */
  private offset = 0;
  /** Keyboard focus keeps the focused option in view; the wheel scrolls freely. */
  private follow = true;

  constructor(private readonly title: string, private readonly summary: string, readonly plan: boolean,
    private readonly actions: SheetAction[], private readonly done: (result: SheetResult) => void,
    private readonly changed: () => void = () => {}, private readonly withContext = true,
    private readonly offContexts: Partial<Record<ContextMode, string>> = {},
    private readonly maxRows: () => number = () => Number.POSITIVE_INFINITY) {}

  /** Take-back/save of board work has no new continuation, so no context choice. */
  private get contexts() { return this.withContext ? CONTEXTS : []; }
  private get rows(): number { return this.contexts.length + this.actions.length + 1; }

  private finish(result: SheetResult): void {
    if (this.finished) return;
    this.finished = true;
    this.done(result);
  }

  /** Activate exactly row `row`; a disabled action or a context row never submits. */
  activate(row: number): void {
    if (this.finished) return;
    this.focus = row;
    const contexts = this.contexts;
    if (row < contexts.length) {
      if (!this.offContexts[contexts[row]!.id]) this.context = contexts[row]!.id;
      return;
    }
    const action = this.actions[row - contexts.length];
    if (!action) return this.finish(undefined);
    if (action.disabled) return;
    this.finish({ action: action.id, context: this.context });
  }

  handleInput(data: string): void {
    if (this.finished) return;
    this.follow = true;
    if (matchesKey(data, "escape")) this.finish(undefined);
    else if (matchesKey(data, "up") || matchesKey(data, "shift+tab")) this.focus = (this.focus + this.rows - 1) % this.rows;
    else if (matchesKey(data, "down") || matchesKey(data, "tab")) this.focus = (this.focus + 1) % this.rows;
    else if (matchesKey(data, "enter") || matchesKey(data, "space")) this.activate(this.focus);
    this.changed();
  }

  /** A left click activates the clicked row: the same path Enter takes on the focused row. */
  handleMouse(event: TuiMouseEvent) {
    if (this.finished) return undefined;
    if (event.type === "wheel") {
      this.follow = false;
      this.offset += Math.sign(event.wheelDelta ?? 0) * 2;
      this.changed();
      return { handled: true };
    }
    const row = this.rowAt.get(event.y);
    if (row === undefined) return undefined;
    // Modified clicks are not a choice; only a plain left click activates, like Enter.
    if (event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl) {
      this.activate(row);
      this.changed();
    }
    return { handled: true };
  }

  render(width: number): string[] {
    const w = Math.max(10, width);
    const fit = (line: string) => truncateToWidth(line, w, "…");
    const wrap = (text: string, indent: number) => wrapTextWithAnsi(text, Math.max(10, w - indent)).map(l => fit(" ".repeat(indent) + l));
    this.rowAt.clear();
    const rowLine = new Map<number, number>();
    const out: string[] = [];
    out.push(...wrap(`Hand off · ${this.title}`, 0));
    if (this.plan) out.push(fit("Plan · No implementation"));
    out.push(...wrap(this.summary, 0), "");
    if (this.withContext) out.push(fit("Context for the next session"));
    const mark = (row: number) => (row === this.focus ? ">" : " ");
    this.contexts.forEach((c, i) => {
      rowLine.set(out.length, i);
      const off = this.offContexts[c.id];
      out.push(fit(`${mark(i)} ${this.context === c.id ? "(•)" : "( )"} ${c.label}${off ? " · unavailable" : ""}`));
      out.push(...wrap(off ?? c.description, 8));
    });
    if (this.withContext) out.push("");
    this.actions.forEach((a, i) => {
      const row = this.contexts.length + i;
      rowLine.set(out.length, row);
      out.push(fit(`${mark(row)} [${a.label}]${a.disabled ? " · unavailable" : ""}`));
      out.push(...wrap(a.disabled ?? a.stop, 6));
    });
    rowLine.set(out.length, this.rows - 1);
    out.push(fit(`${mark(this.rows - 1)} [Cancel]`));
    out.push("", ...wrap("↑/↓ or Tab move · Enter or click choose · Esc cancel (nothing changes)", 0));
    return this.window(out, rowLine, w);
  }

  /** Fit the row budget: keep the focused option and its description visible, mark hidden
   * content above/below, and map pointer lines through the visible window only. */
  private window(out: string[], rowLine: Map<number, number>, w: number): string[] {
    const budget = Math.max(5, Math.floor(this.maxRows()));
    if (out.length <= budget) {
      this.offset = 0;
      for (const [line, row] of rowLine) this.rowAt.set(line, row);
      return out;
    }
    const body = budget - 2; // one indicator line above and below
    if (this.follow) {
      const starts = [...rowLine.keys()].sort((a, b) => a - b);
      const start = [...rowLine].find(([, row]) => row === this.focus)?.[0] ?? 0;
      const next = starts.find(line => line > start) ?? out.length;
      const end = Math.min(next, start + body); // option line plus its description
      if (start < this.offset) this.offset = start;
      if (end > this.offset + body) this.offset = end - body;
    }
    this.offset = Math.max(0, Math.min(this.offset, out.length - body));
    const shown = out.slice(this.offset, this.offset + body);
    for (const [line, row] of rowLine) {
      if (line >= this.offset && line < this.offset + body) this.rowAt.set(line - this.offset + 1, row);
    }
    const above = this.offset > 0 ? `↑ ${this.offset} more` : "";
    const below = out.length - this.offset - body;
    return [truncateToWidth(above, w, "…"), ...shown,
      truncateToWidth(below > 0 ? `↓ ${below} more · scroll or ↑/↓` : "", w, "…")];
  }

  invalidate(): void {}
}

/** Read-only scrollable text (the context preview). Any key that is not a scroll closes it. */
export class PreviewView implements Component {
  private offset = 0;
  constructor(private readonly title: string, private readonly lines: string[], private readonly done: () => void,
    private readonly changed: () => void = () => {}, private readonly maxRows: () => number = () => Number.POSITIVE_INFINITY) {}
  handleInput(data: string): void {
    if (matchesKey(data, "up")) this.offset--;
    else if (matchesKey(data, "down")) this.offset++;
    else if (matchesKey(data, "pageUp")) this.offset -= 10;
    else if (matchesKey(data, "pageDown")) this.offset += 10;
    else return this.done();
    this.changed();
  }
  handleMouse(event: TuiMouseEvent) {
    if (event.type === "wheel") { this.offset += Math.sign(event.wheelDelta ?? 0) * 3; this.changed(); }
    else if (event.type === "click") this.done();
    return { handled: true };
  }
  render(width: number): string[] {
    const w = Math.max(10, width);
    const body = this.lines.flatMap(l => l ? wrapTextWithAnsi(l, w) : [""]).map(l => truncateToWidth(l, w, "…"));
    const room = Math.max(3, Math.floor(this.maxRows()) - 3);
    this.offset = Math.max(0, Math.min(this.offset, Math.max(0, body.length - room)));
    const shown = body.slice(this.offset, this.offset + room);
    const more = body.length - this.offset - shown.length;
    return [truncateToWidth(`${this.title}${this.offset ? ` · ↑ ${this.offset} more` : ""}`, w, "…"), ...shown,
      truncateToWidth(`${more > 0 ? `↓ ${more} more · ` : ""}↑/↓ PgUp/PgDn or wheel scroll · any other key closes`, w, "…")];
  }
  invalidate(): void {}
}
