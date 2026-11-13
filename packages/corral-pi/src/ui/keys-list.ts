// F1: the keys this mode offers, grouped. Rows that carry a key sequence can be run: ↑↓ selects among
// them, Enter or a click returns the sequence, Esc or F1 closes. Reference rows only inform.
import { matchesKey, truncateToWidth, visibleWidth, type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";
import { keyCap } from "./header.ts";
import type { KeyGroup, KeyRow } from "./keymap.ts";
import { pad, plainLabel, PANEL_BG, PANEL_SELECTED, RESET } from "./panels-view.ts";

type Line = { readonly kind: "group"; readonly title: string } | { readonly kind: "row"; readonly row: KeyRow };

export class KeysList implements Component, Focusable {
  focused = false;
  private readonly lines: readonly Line[];
  private readonly runnable: readonly number[];
  private selected: number;
  private start = 0;
  constructor(groups: readonly KeyGroup[], private readonly theme: Pick<Theme, "fg" | "bold">,
    private readonly done: (data: string | undefined) => void, private readonly changed: () => void,
    private readonly capacity: number) {
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
    if (event.y >= 1) this.run(this.start + event.y - 1);
    return { handled: true };
  }
  private run(index: number): void {
    const line = this.lines[index];
    if (line?.kind === "row" && line.row.data) this.done(line.row.data);
  }
  render(width: number): string[] {
    const room = Math.max(0, width);
    const height = Math.max(1, this.capacity);
    if (this.selected >= 0) {
      // Keep the selection in view with the least movement; show its group title when it opens a group.
      const top = this.lines[this.selected - 1]?.kind === "group" ? this.selected - 1 : this.selected;
      if (top < this.start) this.start = top;
      if (this.selected >= this.start + height) this.start = this.selected - height + 1;
    }
    this.start = Math.max(0, Math.min(this.start, this.lines.length - height));
    const keyWidth = Math.min(14, Math.max(0, ...this.lines.map(line => line.kind === "row" ? visibleWidth(plainLabel(line.row.key)) + 2 : 0)));
    const out = [truncateToWidth(this.theme.bold("Keys"), room, "…")];
    for (const [offset, line] of this.lines.slice(this.start, this.start + height).entries()) {
      if (line.kind === "group") {
        out.push(truncateToWidth("  " + this.theme.bold(this.theme.fg("muted", plainLabel(line.title).toUpperCase())), room, "…"));
        continue;
      }
      const on = this.start + offset === this.selected;
      const bg = on ? PANEL_SELECTED : PANEL_BG;
      const key = plainLabel(line.row.key);
      const cap = keyCap(key, { restore: bg }) + " ".repeat(Math.max(0, keyWidth - visibleWidth(key) - 2));
      const text = (on ? this.theme.fg("accent", "▌") : " ") + " " + cap + " " + this.theme.fg("text", plainLabel(line.row.label));
      out.push(bg + pad(text, room).replaceAll(RESET, RESET + bg) + RESET);
    }
    return out;
  }
  invalidate(): void {}
}
