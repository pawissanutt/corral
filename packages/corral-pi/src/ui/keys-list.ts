// F1: the keys this mode offers, grouped, as a popup on the raised surface. Rows that carry a key sequence
// can be run: ↑↓ selects among them, Enter or a click returns the sequence, Esc, F1 or a click on
// `esc close` closes. Reference rows only inform.
import { matchesKey, visibleWidth, type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { keyCap, popup } from "./frame.ts";
import type { KeyGroup, KeyRow } from "./keymap.ts";
import { plainLabel, PANEL_HEAD, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

type Line = { readonly kind: "group"; readonly title: string } | { readonly kind: "row"; readonly row: KeyRow };
/** Rows above the list: the top border, the title row and its divider. */
const HEAD_ROWS = 3;
/** Cells of the key column: the cap and its gap before the label. */
const KEY_COLUMN = 11;
/** `[esc] close` at the right of the title row; built per render so an appearance change recolours it. */
function closeLabel(): string { return keyCap("esc") + " " + PANEL_MUTED + "close"; }

export class KeysList implements Component, Focusable {
  focused = false;
  private readonly lines: readonly Line[];
  private readonly runnable: readonly number[];
  private selected: number;
  private start = 0;
  private width = 0;
  constructor(groups: readonly KeyGroup[], private readonly done: (data: string | undefined) => void,
    private readonly changed: () => void, private readonly capacity: number) {
    this.lines = groups.flatMap(group => [{ kind: "group" as const, title: group.title },
      ...group.rows.map(row => ({ kind: "row" as const, row }))]);
    this.runnable = this.lines.flatMap((line, index) => line.kind === "row" && line.row.data ? [index] : []);
    this.selected = this.runnable[0] ?? -1;
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "f1")) { this.done(undefined); return; }
    if (matchesKey(data, "enter")) { this.run(this.selected); return; }
    const at = this.runnable.indexOf(this.selected);
    if (matchesKey(data, "up") && at > 0) this.selected = this.runnable[at - 1]!;
    else if (matchesKey(data, "down") && at >= 0 && at < this.runnable.length - 1) this.selected = this.runnable[at + 1]!;
    else return;
    this.changed();
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return { handled: true };
    // `esc close` sits at the right end of the title row.
    if (event.y === 1 && event.x >= this.width - 2 - visibleWidth(closeLabel())) { this.done(undefined); return { handled: true }; }
    if (event.y >= HEAD_ROWS) this.run(this.start + event.y - HEAD_ROWS);
    return { handled: true };
  }
  private run(index: number): void {
    const line = this.lines[index];
    if (line?.kind === "row" && line.row.data) this.done(line.row.data);
  }
  render(width: number): string[] {
    this.width = width;
    const height = Math.max(1, this.capacity);
    if (this.selected >= 0) {
      // Keep the selection in view with the least movement; show its group title when it opens a group.
      const top = this.lines[this.selected - 1]?.kind === "group" ? this.selected - 1 : this.selected;
      if (top < this.start) this.start = top;
      if (this.selected >= this.start + height) this.start = this.selected - height + 1;
    }
    this.start = Math.max(0, Math.min(this.start, this.lines.length - height));
    const shown = this.lines.slice(this.start, this.start + height);
    const body = shown.map(line => {
      if (line.kind === "group") return " " + PANEL_HEAD + "\x1b[1m" + plainLabel(line.title) + "\x1b[22m" + PANEL_TEXT;
      const key = plainLabel(line.row.key);
      return " " + keyCap(key) + " ".repeat(Math.max(1, KEY_COLUMN - visibleWidth(key) - 2)) + PANEL_TEXT + plainLabel(line.row.label);
    });
    const at = this.selected - this.start;
    return popup("\x1b[1mKeys\x1b[22m", closeLabel(), body, width, at >= 0 && at < shown.length ? at : undefined);
  }
  invalidate(): void {}
}
