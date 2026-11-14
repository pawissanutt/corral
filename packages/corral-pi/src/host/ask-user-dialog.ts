// The owner's question dialog: every question of one ask_user call in one component, one tab per
// question plus Submit. Questions are answered in any order and revisable until Submit; Escape (or
// the caller's timeout) ends the dialog keeping whatever was answered, so a partial batch survives.
// Unless freeText is false, an Other row takes the owner's own typed answer instead of an option;
// with freeText true that row is a note on the chosen option. Drafts stay per question.
import { Input, matchesKey, type Component, type Focusable, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { askDialogFrame, type AskDialogFrame, type AskDialogKey, type AskDialogText, type AskDialogView } from "../ui/ask-dialog-view.ts";
import { askDialogPlainLines } from "../ui/ask-dialog-plain.ts";
import { choiceRows } from "../ui/build/choice-dock.ts";

export type Choice = { label: string; description?: string };
export type DialogQuestion = { question: string; header?: string; options: Choice[]; multiSelect?: boolean; freeText?: boolean };
/** `selected` is empty for the owner's own typed answer, which is then the nonempty `text`. */
export type DialogAnswer = { selected: string[]; text: string };
export type DialogResult = { submitted: boolean; answers: (DialogAnswer | undefined)[] };
/**
 * `drafts` seeds each text row and receives every edit, so a caller can keep drafts across reopenings. `surface` is
 * internal, never an SDK option: "plain" keeps the stock text dialog for hosts without native panels (default
 * "panels"); `rows` bounds the panels card only.
 */
export type DialogOptions = { escape?: string; complete?: boolean; drafts?: string[]; rows?: () => number; surface?: "panels" | "plain" };
/** Rows the rest of the screen keeps while the card is open, when the host gave no row figure of its own. */
const CHROME_ROWS = 6;
/** Body rows one wheel notch moves. */
const WHEEL_ROWS = 3;

const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";
const flatten = (text: string) => text.replace(/\r\n|\r|\n/g, " ").replace(/[\x00-\x1f\x7f]/g, "");

/** Line breaks inside a bracketed paste become spaces, so a pasted multi-line answer is kept whole
 * and never read as Enter. A paste may span several input events. */
function pasteChunk(data: string, pasting: boolean): { data: string; pasting: boolean } {
  let out = "", rest = data, inside = pasting;
  while (rest) {
    if (inside) {
      const end = rest.indexOf(PASTE_END);
      out += flatten(end < 0 ? rest : rest.slice(0, end));
      if (end < 0) break;
      out += PASTE_END; rest = rest.slice(end + PASTE_END.length); inside = false;
    } else {
      const start = rest.indexOf(PASTE_START);
      if (start < 0) { out += rest; break; }
      out += rest.slice(0, start + PASTE_START.length); rest = rest.slice(start + PASTE_START.length); inside = true;
    }
  }
  return { data: out, pasting: inside };
}

export class AskDialog implements Component, Focusable {
  tab = 0;
  cursor = 0;
  focused = false;
  readonly answers: (DialogAnswer | undefined)[];
  private readonly picks: Set<string>[];
  private readonly inputs: Input[];
  private pasting = false;
  private finished = false;
  private hint: AskDialogFrame["hint"] = { row: -1, hits: [] };
  /** The first body row the owner paged to; undefined while the body follows the cursor. */
  private scroll: number | undefined;
  private window: AskDialogFrame["scroll"] = { start: 0, max: 0, page: 1 };

  /** `complete` keeps Submit until every question is answered; `escape` names what Esc does. */
  constructor(private readonly questions: DialogQuestion[], private readonly done: (result: DialogResult) => void,
    private readonly changed: () => void = () => {}, private readonly options: DialogOptions = {}) {
    this.answers = questions.map(() => undefined);
    this.picks = questions.map(() => new Set());
    this.inputs = questions.map((_, i) => {
      const input = new Input({ prompt: "" });
      const draft = options.drafts?.[i];
      if (draft) { input.setValue(draft); input.handleInput("\x05"); } // caret at the end
      return input;
    });
  }

  /** Ends the dialog once; answered questions are kept whether submitted or not. */
  finish(submitted: boolean): void {
    if (this.finished) return;
    this.finished = true;
    this.done({ submitted, answers: [...this.answers] });
  }

  private get onSubmit(): boolean { return this.tab === this.questions.length; }
  private rows(q: DialogQuestion): number { return q.options.length + (q.freeText === false ? 0 : 1); }
  private note(i: number): string { return this.inputs[i]!.getValue(); }
  private picked(i: number): string[] {
    return this.questions[i]!.options.map((o) => o.label).filter((l) => this.picks[i]!.has(l));
  }
  /** The text row annotates a chosen option only for freeText questions; otherwise it is Other. */
  private noting(i: number): boolean {
    const q = this.questions[i]!;
    return q.freeText === true && (!!this.answers[i]?.selected.length || (!!q.multiSelect && this.picks[i]!.size > 0));
  }

  private go(tab: number): void {
    this.tab = Math.max(0, Math.min(this.questions.length, tab));
    this.cursor = 0;
  }

  /** After an answer, move to the next unanswered question, or Submit once all are answered. */
  private advance(): void {
    const next = this.answers.findIndex((a, i) => a === undefined && i > this.tab);
    const any = this.answers.findIndex((a) => a === undefined);
    this.go(next >= 0 ? next : any >= 0 ? any : this.questions.length);
  }

  private confirm(i: number, selected: string[]): void {
    if (selected.length === 0) return;
    this.answers[i] = { selected, text: this.questions[i]!.freeText === true ? this.note(i) : "" };
    this.advance();
  }

  handleInput(data: string): void {
    if (this.finished) return;
    // PgUp/PgDn page a clipped body without touching any answer; every other key is navigation or typing, which
    // lets the body follow the cursor again.
    if (this.options.surface !== "plain" && !this.pasting && (matchesKey(data, "pageUp") || matchesKey(data, "pageDown"))) {
      this.scrollBy(matchesKey(data, "pageUp") ? -this.window.page : this.window.page);
      this.changed();
      return;
    }
    this.scroll = undefined;
    this.handle(data);
    this.changed();
  }

  private handle(data: string): void {
    const q = this.questions[this.tab];
    const onText = q !== undefined && this.cursor === q.options.length;
    if (onText && (this.pasting || data.includes(PASTE_START))) {
      const chunk = pasteChunk(data, this.pasting);
      this.pasting = chunk.pasting;
      return this.edit(chunk.data);
    }
    if (matchesKey(data, "escape")) return this.finish(false);
    if (matchesKey(data, "tab")) return this.go(this.tab + 1);
    if (matchesKey(data, "shift+tab")) return this.go(this.tab - 1);
    // On the text row ←/→ move the caret; elsewhere they switch questions.
    if (!onText && matchesKey(data, "right")) return this.go(this.tab + 1);
    if (!onText && matchesKey(data, "left")) return this.go(this.tab - 1);
    if (this.onSubmit || q === undefined) {
      if (!matchesKey(data, "enter")) return;
      const missing = this.answers.findIndex((a) => a === undefined);
      if (this.options.complete && missing >= 0) return this.go(missing);
      return this.finish(true);
    }
    const i = this.tab;
    if (matchesKey(data, "up")) { this.cursor = Math.max(0, this.cursor - 1); return; }
    if (matchesKey(data, "down")) { this.cursor = Math.min(this.rows(q) - 1, this.cursor + 1); return; }
    const option = q.options[this.cursor];
    if (option === undefined) {
      if (!matchesKey(data, "enter") && data !== "\n") {
        // Fast typing or an unbracketed paste arrives as one chunk; its line breaks are text.
        return this.edit(!data.includes("\x1b") && data.length > 1 ? flatten(data) : data);
      }
      // Enter saves a note on the chosen option, or uses the typed text as the answer.
      const answer = this.answers[i];
      const own = this.note(i).trim();
      const picked = answer?.selected.length ? answer.selected : q.multiSelect ? this.picked(i) : [];
      if (this.noting(i)) return this.confirm(i, picked);
      if (own) {
        this.picks[i]!.clear();
        this.answers[i] = { selected: [], text: own };
        return this.advance();
      }
      if (answer) return this.advance();
      if (picked.length) return this.confirm(i, picked);
      this.cursor = 0;
      return;
    }
    if (q.multiSelect) {
      const picks = this.picks[i]!;
      if (matchesKey(data, "space")) {
        if (picks.has(option.label)) picks.delete(option.label); else picks.add(option.label);
        if (this.answers[i]) this.answers[i] = picks.size ? { selected: this.picked(i), text: q.freeText === true ? this.note(i) : "" } : undefined;
      } else if (matchesKey(data, "enter")) {
        if (picks.size === 0) picks.add(option.label);
        this.confirm(i, this.picked(i));
      }
      return;
    }
    // Space on the chosen option clears it, so the text row can become the answer instead.
    if (matchesKey(data, "space") && this.answers[i]?.selected[0] === option.label) { this.answers[i] = undefined; return; }
    if (matchesKey(data, "enter") || matchesKey(data, "space")) this.confirm(i, [option.label]);
  }

  /** Text-row editing goes to the input; a typed answer or note follows the draft. */
  private edit(data: string): void {
    const i = this.tab;
    if (data) this.inputs[i]!.handleInput(data);
    if (this.options.drafts) this.options.drafts[i] = this.note(i);
    const answer = this.answers[i];
    if (!answer) return;
    if (answer.selected.length === 0) {
      const own = this.note(i).trim();
      this.answers[i] = own ? { selected: [], text: own } : undefined;
    } else if (this.questions[i]!.freeText === true) answer.text = this.note(i);
  }

  /** Read-only state for the answer card; reading it changes nothing in the dialog. */
  view(): AskDialogView {
    const q = this.questions[this.tab];
    const label = (item: DialogQuestion, i: number) => item.header ?? `Q${i + 1}`;
    const tabs = [
      ...this.questions.map((item, i) => ({ label: label(item, i), answered: this.answers[i] !== undefined, active: i === this.tab, submit: false })),
      { label: "Review", answered: false, active: this.onSubmit, submit: true },
    ];
    const esc = { key: "esc", label: (this.options.escape ?? "Esc send answered only").replace(/^esc\s+/i, ""), data: "\x1b" };
    if (this.onSubmit || q === undefined) {
      // Answers are shown in full here: this is where the owner inspects them before sending.
      const unanswered = this.answers.filter((a) => a === undefined).length;
      const required = this.options.complete === true;
      const rows = this.questions.map((item, i) => {
        const a = this.answers[i];
        const answer = !a ? undefined : a.selected.length ? a.selected.join(", ") + (a.text ? ` (${a.text})` : "") : `Other: ${a.text}`;
        return { label: label(item, i), answer };
      });
      return { tabs, review: { rows, unanswered, required }, keys: [
        { key: "⏎", label: required && unanswered > 0 ? "first unanswered" : "send answers", data: "\r" }, esc,
        { key: "←", label: "revise", data: "\x1b[D" },
      ] };
    }
    const answer = this.answers[this.tab];
    const onText = this.cursor === q.options.length;
    const noting = this.noting(this.tab);
    const input = this.inputs[this.tab]!;
    const own: AskDialogText | undefined = q.freeText === false ? undefined : {
      kind: noting ? "note" : "other", cursor: onText, on: !noting && answer !== undefined && answer.selected.length === 0, draft: input.getValue(),
      renderInput: (width) => { input.focused = this.focused; return input.render(Math.max(1, width))[0] ?? ""; },
    };
    const rows = this.rows(q);
    const chosen = !q.multiSelect && answer?.selected.length && answer.selected[0] === q.options[this.cursor]?.label;
    const space = q.multiSelect ? "toggle" : chosen ? "clear" : undefined;
    // ↑↓ steps the way that still has a row; on the text row ←/→ are the caret's, so Tab leaves it.
    const keys: AskDialogKey[] = [
      { key: "⏎", label: onText ? (noting ? "save note" : "use answer") : q.multiSelect ? "confirm" : "choose", data: "\r" },
      ...(!onText && space ? [{ key: "Space", label: space, data: " " }] : []),
      esc,
      onText ? { key: "Tab", label: "questions", data: "\t" } : { key: "←→", label: "questions", data: "\x1b[C" },
      ...(rows > 1 ? [{ key: "↑↓", label: "select", data: this.cursor < rows - 1 ? "\x1b[B" : "\x1b[A" }] : []),
    ];
    return { tabs, keys, question: {
      prompt: q.question, multiSelect: q.multiSelect === true,
      options: q.options.map((o, row) => ({
        label: o.label, ...(o.description ? { description: o.description } : {}), cursor: row === this.cursor,
        on: q.multiSelect ? this.picks[this.tab]!.has(o.label) : answer?.selected[0] === o.label,
      })),
      ...(own ? { own } : {}),
    } };
  }

  render(width: number): string[] {
    if (this.options.surface === "plain") return askDialogPlainLines(this.view(), this.options.escape ?? "Esc send answered only", width);
    const frame = askDialogFrame(this.view(), width, this.budget(), this.scroll);
    this.hint = frame.hint;
    this.window = frame.scroll;
    return frame.lines;
  }

  /** The card's own rows: the host's figure, else a share of the terminal; unbounded when neither is known. */
  private budget(): number {
    const rows = this.options.rows?.() ?? (process.stdout.rows ? choiceRows(process.stdout.rows, CHROME_ROWS) : undefined);
    return rows ?? Number.POSITIVE_INFINITY;
  }

  /** Pages the clipped body from where it is now; nothing happens while nothing is clipped. */
  private scrollBy(rows: number): void {
    if (this.window.max === 0) return;
    this.scroll = Math.max(0, Math.min(this.window.max, (this.scroll ?? this.window.start) + rows));
  }

  /**
   * A click on a key cap replays that key through the dialog's own input; the wheel pages a clipped body. Plain hosts
   * draw no caps and keep the stock behaviour.
   */
  handleMouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    if (this.options.surface === "plain") return undefined;
    if (event.type === "wheel" && event.wheelDelta) {
      this.scrollBy(Math.sign(event.wheelDelta) * WHEEL_ROWS);
      this.changed();
      return { handled: true };
    }
    if (event.type !== "click" || event.button !== "left" || event.shift || event.ctrl || event.alt || event.y !== this.hint.row) return undefined;
    const hit = this.hint.hits.find((h) => event.x >= h.from && event.x < h.to);
    if (!hit) return undefined;
    this.handleInput(hit.data);
    return { handled: true };
  }

  invalidate(): void {}
}
