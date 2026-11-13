// F1: the keys this mode offers, grouped, as a popup on the raised surface. The window scrolls by line over every
// line, reference groups included: ↑↓ select among the rows that carry a key sequence and scroll the window when
// the next one is off screen or there is none, PgUp/PgDn, Home/End and the wheel scroll the window, and the title
// row shows `n–m / total`. Enter or a click returns the selected sequence; Esc, F1 or a click on `esc close` closes.
import { matchesKey, visibleWidth, type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { keyCap, popup } from "./frame.ts";
import type { KeyGroup, KeyRow } from "./keymap.ts";
import { pagerPosition } from "./pager.ts";
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
  /** The selected runnable line; it is drawn and runs only while it is in the window. */
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
  private get height(): number { return Math.max(1, Math.min(this.capacity, this.lines.length)); }
  private visible(index: number): boolean { return index >= this.start && index < this.start + this.height; }
  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "f1")) { this.done(undefined); return; }
    if (matchesKey(data, "enter")) { if (this.visible(this.selected)) this.run(this.selected); return; }
    const page = Math.max(1, this.height - 1);
    if (matchesKey(data, "down")) this.step(1);
    else if (matchesKey(data, "up")) this.step(-1);
    else if (matchesKey(data, "pageDown")) this.scroll(page);
    else if (matchesKey(data, "pageUp")) this.scroll(-page);
    else if (matchesKey(data, "home")) this.scroll(-this.lines.length);
    else if (matchesKey(data, "end")) this.scroll(this.lines.length);
    else return;
    this.changed();
  }
  /**
   * ↓ or ↑: the next runnable row in that direction when it is in the window, else one line of scroll. A selection
   * scrolled out of the window counts from the window's edge instead.
   */
  private step(direction: 1 | -1): void {
    const shown = this.visible(this.selected);
    const next = direction > 0
      ? this.runnable.find(index => shown ? index > this.selected : index >= this.start)
      : this.runnable.findLast(index => shown ? index < this.selected : index < this.start + this.height);
    if (next !== undefined && this.visible(next)) { this.selected = next; return; }
    this.scroll(direction);
  }
  /** Moves the window; a selection it leaves moves to the nearest runnable row still in it, when there is one. */
  private scroll(delta: number): void {
    this.start = Math.max(0, Math.min(this.lines.length - this.height, this.start + delta));
    if (this.visible(this.selected)) return;
    const inside = this.runnable.filter(index => this.visible(index));
    const nearest = delta > 0 ? inside[0] : inside.at(-1);
    if (nearest !== undefined) this.selected = nearest;
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type === "wheel") {
      if (event.wheelDelta) { this.scroll(event.wheelDelta); this.changed(); }
      return { handled: true };
    }
    if (event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return { handled: true };
    // `esc close` sits at the right end of the title row.
    if (event.y === 1 && event.x >= this.width - 2 - visibleWidth(closeLabel())) { this.done(undefined); return { handled: true }; }
    if (event.y >= HEAD_ROWS && event.y < HEAD_ROWS + this.height) this.run(this.start + event.y - HEAD_ROWS);
    return { handled: true };
  }
  private run(index: number): void {
    const line = this.lines[index];
    if (line?.kind === "row" && line.row.data) this.done(line.row.data);
  }
  render(width: number): string[] {
    this.width = width;
    const height = this.height;
    this.start = Math.max(0, Math.min(this.start, this.lines.length - height));
    const shown = this.lines.slice(this.start, this.start + height);
    const body = shown.map(line => {
      if (line.kind === "group") return " " + PANEL_HEAD + "\x1b[1m" + plainLabel(line.title) + "\x1b[22m" + PANEL_TEXT;
      const key = plainLabel(line.row.key);
      return " " + keyCap(key) + " ".repeat(Math.max(1, KEY_COLUMN - visibleWidth(key) - 2)) + PANEL_TEXT + plainLabel(line.row.label);
    });
    const position = PANEL_MUTED + pagerPosition(this.start, this.start + shown.length - 1, this.lines.length) + "  " + PANEL_TEXT;
    // `Keys` and `esc close` stay; the position goes first when the title row is too narrow for all three.
    const right = visibleWidth(position + closeLabel()) + 6 <= width - 8 ? position + closeLabel() : closeLabel();
    const at = this.selected - this.start;
    return popup("\x1b[1mKeys\x1b[22m", right, body, width, at >= 0 && at < shown.length ? at : undefined);
  }
  invalidate(): void {}
}
