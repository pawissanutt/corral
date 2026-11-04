// The owner's question dialog: every question of one ask_user call in one component, one tab per
// question plus Submit. Questions are answered in any order and revisable until Submit; Escape (or
// the caller's timeout) ends the dialog keeping whatever was answered, so a partial batch survives.
import { matchesKey, truncateToWidth, wrapTextWithAnsi, type Component } from "@earendil-works/pi-tui";

export type Choice = { label: string; description?: string };
export type DialogQuestion = { question: string; header?: string; options: Choice[]; multiSelect?: boolean; freeText?: boolean };
export type DialogAnswer = { selected: string[]; text: string };
export type DialogResult = { submitted: boolean; answers: (DialogAnswer | undefined)[] };

const printable = (data: string) => data.length === 1 && data >= " " && data !== "\x7f";

export class AskDialog implements Component {
  tab = 0;
  cursor = 0;
  readonly answers: (DialogAnswer | undefined)[];
  private readonly picks: Set<string>[];
  private readonly notes: string[];
  private finished = false;

  constructor(private readonly questions: DialogQuestion[], private readonly done: (result: DialogResult) => void,
    private readonly changed: () => void = () => {}) {
    this.answers = questions.map(() => undefined);
    this.picks = questions.map(() => new Set());
    this.notes = questions.map(() => "");
  }

  /** Ends the dialog once; answered questions are kept whether submitted or not. */
  finish(submitted: boolean): void {
    if (this.finished) return;
    this.finished = true;
    this.done({ submitted, answers: [...this.answers] });
  }

  private get onSubmit(): boolean { return this.tab === this.questions.length; }
  private rows(q: DialogQuestion): number { return q.options.length + (q.freeText ? 1 : 0); }

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
    this.answers[i] = { selected, text: this.notes[i] ?? "" };
    this.advance();
  }

  handleInput(data: string): void {
    if (this.finished) return;
    this.handle(data);
    this.changed();
  }

  private handle(data: string): void {
    if (matchesKey(data, "escape")) return this.finish(false);
    if (matchesKey(data, "tab") || matchesKey(data, "right")) return this.go(this.tab + 1);
    if (matchesKey(data, "shift+tab") || matchesKey(data, "left")) return this.go(this.tab - 1);
    if (this.onSubmit) {
      if (matchesKey(data, "enter")) this.finish(true);
      return;
    }
    const i = this.tab;
    const q = this.questions[i]!;
    if (matchesKey(data, "up")) { this.cursor = Math.max(0, this.cursor - 1); return; }
    if (matchesKey(data, "down")) { this.cursor = Math.min(this.rows(q) - 1, this.cursor + 1); return; }
    const option = q.options[this.cursor];
    if (option === undefined) {
      // The note row of a free-text question: typing edits it, Enter confirms the question.
      const answer = this.answers[i];
      if (matchesKey(data, "backspace")) this.notes[i] = (this.notes[i] ?? "").slice(0, -1);
      else if (matchesKey(data, "enter")) {
        if (answer) this.advance();
        else if (!q.multiSelect) this.cursor = 0;
        else this.confirm(i, q.options.map((o) => o.label).filter((l) => this.picks[i]!.has(l)));
        return;
      } else if (printable(data)) this.notes[i] = (this.notes[i] ?? "") + data;
      if (answer) answer.text = this.notes[i] ?? "";
      return;
    }
    if (q.multiSelect) {
      const picks = this.picks[i]!;
      if (matchesKey(data, "space")) {
        if (picks.has(option.label)) picks.delete(option.label); else picks.add(option.label);
        if (this.answers[i]) this.answers[i] = picks.size ? { selected: q.options.map((o) => o.label).filter((l) => picks.has(l)), text: this.notes[i] ?? "" } : undefined;
      } else if (matchesKey(data, "enter")) {
        if (picks.size === 0) picks.add(option.label);
        this.confirm(i, q.options.map((o) => o.label).filter((l) => picks.has(l)));
      }
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "space")) this.confirm(i, [option.label]);
  }

  render(width: number): string[] {
    const fit = (line: string) => truncateToWidth(line, Math.max(1, width));
    const label = (q: DialogQuestion, i: number) => q.header ?? `Q${i + 1}`;
    const tabs = this.questions.map((q, i) => `${this.answers[i] ? "✓" : "○"} ${label(q, i)}`).concat("Submit")
      .map((t, i) => (i === this.tab ? `[${t}]` : ` ${t} `)).join(" ");
    const out = [fit(tabs), ""];
    if (this.onSubmit) {
      this.questions.forEach((q, i) => {
        const a = this.answers[i];
        out.push(fit(`${label(q, i)}: ${a ? a.selected.join(", ") + (a.text ? ` (${a.text})` : "") : "unanswered"}`));
      });
      out.push("", fit("Enter submit · ←/→ revise · Esc send answered only"));
      return out;
    }
    const q = this.questions[this.tab]!;
    out.push(...wrapTextWithAnsi(q.question, Math.max(10, width)).map(fit), "");
    const answer = this.answers[this.tab];
    q.options.forEach((o, row) => {
      const on = q.multiSelect ? this.picks[this.tab]!.has(o.label) : answer?.selected[0] === o.label;
      const box = q.multiSelect ? (on ? "[x]" : "[ ]") : on ? "(•)" : "( )";
      out.push(fit(`${row === this.cursor ? ">" : " "} ${box} ${o.label}`));
      if (o.description) out.push(...wrapTextWithAnsi(o.description, Math.max(10, width - 8)).map((l) => fit(`        ${l}`)));
    });
    if (q.freeText) out.push(fit(`${this.cursor === q.options.length ? ">" : " "} Note: ${this.notes[this.tab] ?? ""}_`));
    out.push("", fit(`↑/↓ move · ${q.multiSelect ? "Space toggle · Enter confirm" : "Enter choose"} · ←/→ questions · Esc send answered only`));
    return out;
  }

  invalidate(): void {}
}
