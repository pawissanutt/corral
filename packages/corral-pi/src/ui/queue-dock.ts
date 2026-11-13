// Queued input above the composer: the owner's own items can be pulled back, reordered or removed.
import { decodeKittyPrintable, matchesKey, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent } from "@earendil-works/pi-tui";
import type { QueueControl, QueuedItem } from "../host/queue-control.ts";
import { plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SELECTED, PANEL_TEXT, RESET } from "./panels-view.ts";

export interface QueueDockHost {
  items(): readonly QueuedItem[];
  control(): QueueControl | undefined;
  draft(): string; setDraft(text: string): void;
  focusComposer(): void;
  notice(text: string, tone: "text" | "warning"): void;
  changed(): void;
}

/** Below this many columns the controls shrink to glyphs and the header to its count. */
const TIGHT_COLUMNS = 84;
const POLICY_CELLS = 6;
const OFF = "Queue control is off in this session";
const GONE = "That input already left the queue";

/** Item rows the dock may use: 5, 3 below 28 terminal rows, 2 below 20. */
export function queueCapacity(rows: number): number {
  return rows < 20 ? 2 : rows < 28 ? 3 : 5;
}

type QueueOp = "edit" | "remove" | -1 | 1;
interface QueueHit { readonly from: number; readonly to: number; readonly key?: string; readonly op?: QueueOp | "select" }

/** Plain text clipped to `width` cells with an ellipsis, then padded; no reset codes inside. */
function fit(text: string, width: number): string {
  if (width <= 0) return "";
  const out = visibleWidth(text) > width ? truncateToWidth(text, width - 1, "").replaceAll(RESET, "") + "…" : text;
  return out + " ".repeat(Math.max(0, width - visibleWidth(out)));
}

export class QueueDock implements Component {
  focused = false;
  selected: string | undefined;
  private capacity = 5;
  private offset = 0;
  private index = 0;
  private pending: string | undefined;
  private hits = new Map<number, readonly QueueHit[]>();

  constructor(private readonly host: QueueDockHost) {}

  /** True while one queue operation awaits the host; others are ignored until it settles. */
  get busy(): boolean { return this.pending !== undefined; }
  /** Controls are offered only while the host's queue control is enabled for this session. */
  get controllable(): boolean { return this.host.control()?.enabled() === true; }

  setCapacity(rows: number): void { this.capacity = queueCapacity(rows); }

  /** Selects the last editable item and takes queue focus; false when nothing is editable. */
  focusLast(): boolean {
    const last = this.editable().at(-1);
    if (!last) return false;
    this.selected = last.key; this.focused = true; this.host.changed();
    return true;
  }

  /** Pulls the last editable item back into the composer, which keeps focus. */
  async editLast(): Promise<boolean> {
    const last = this.editable().at(-1);
    return last ? this.operate(last.key, "edit") : false;
  }

  handleKey(data: string): boolean {
    if (!this.focused) return false;
    const ch = decodeKittyPrintable(data) ?? data;
    const op = (kind: QueueOp) => { const key = this.current()?.key; if (key) void this.operate(key, kind); return true; };
    if (matchesKey(data, "escape") || matchesKey(data, "tab")) { this.leave(); return true; }
    if (matchesKey(data, "shift+up") || ch === "K") return op(-1);
    if (matchesKey(data, "shift+down") || ch === "J") return op(1);
    if (matchesKey(data, "up") || ch === "k") { this.step(-1); return true; }
    if (matchesKey(data, "down") || ch === "j") { this.step(1); return true; }
    if (matchesKey(data, "enter") || ch === "e") return op("edit");
    if (matchesKey(data, "delete") || ch === "x") return op("remove");
    // Terminal replies and unbound special keys are not typing; they keep queue focus.
    if (ch.startsWith("\x1b")) return false;
    this.leave();
    return false;
  }

  render(width: number): string[] {
    this.hits = new Map();
    const items = this.host.items();
    if (!items.length || width <= 0) return [];
    const editable = items.filter(item => item.editable);
    const current = this.resolve(editable);
    if (!editable.length) this.focused = false;
    const tight = width < TIGHT_COLUMNS, shown = Math.min(this.capacity, editable.length);
    // The window moves only as far as it must to keep the selection in view.
    if (current !== undefined && current < this.offset) this.offset = current;
    else if (current !== undefined && current >= this.offset + shown) this.offset = current - shown + 1;
    this.offset = Math.max(0, Math.min(this.offset, editable.length - shown));
    const lines = [this.header(items.length, editable.length - shown, tight, width)];
    for (let at = this.offset; at < this.offset + shown; at++) {
      lines.push(this.row(editable[at]!, at, editable.length, tight, width, lines.length));
    }
    const notices = items.length - editable.length;
    if (notices) {
      this.hits.set(lines.length, [{ from: 0, to: width }]);
      lines.push("  " + PANEL_MUTED + fit(`+ ${notices} queued notice${notices === 1 ? "" : "s"} · read-only`, width - 4) + "  " + RESET);
    }
    // Rows are built to exactly `width`; only a terminal narrower than the margins clips them.
    return width < 12 ? lines.map(line => truncateToWidth(line, width, "")) : lines;
  }

  handleMouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    const hits = this.hits.get(event.y);
    if (!hits) return undefined;
    if (event.type !== "click" || event.button !== "left") return { handled: true };
    const hit = hits.find(h => event.x >= h.from && event.x < h.to);
    if (!hit?.key || !hit.op) return { handled: true };
    this.selected = hit.key;
    if (hit.op === "select") { this.focused = true; this.host.changed(); }
    else void this.operate(hit.key, hit.op);
    return { handled: true };
  }

  invalidate(): void {}

  private editable(): QueuedItem[] { return this.host.items().filter(item => item.editable); }

  /** Keeps `selected` on an editable key: a vanished item hands the selection to its neighbour. */
  private resolve(editable: readonly QueuedItem[]): number | undefined {
    if (this.selected === undefined) return undefined;
    const at = editable.findIndex(item => item.key === this.selected);
    if (at >= 0) { this.index = at; return at; }
    if (!editable.length) { this.selected = undefined; return undefined; }
    this.index = Math.min(this.index, editable.length - 1);
    this.selected = editable[this.index]!.key;
    return this.index;
  }

  private current(): QueuedItem | undefined {
    const editable = this.editable(), at = this.resolve(editable);
    return at === undefined ? editable.at(-1) : editable[at];
  }

  private step(delta: number): void {
    const editable = this.editable();
    if (!editable.length) return;
    this.index = Math.max(0, Math.min(editable.length - 1, (this.resolve(editable) ?? editable.length - 1) + delta));
    this.selected = editable[this.index]!.key;
    this.host.changed();
  }

  private leave(): void {
    this.focused = false;
    this.host.focusComposer();
    this.host.changed();
  }

  /**
   * One host call at a time, only for a listed editable key. The row shows `…` while it runs. A
   * refusal (false, undefined or a throw) leaves the list to the next snapshot and raises a notice.
   */
  private async operate(key: string, op: QueueOp): Promise<boolean> {
    if (this.busy) return false;
    const editable = this.editable(), at = editable.findIndex(item => item.key === key);
    if (at < 0 || (typeof op === "number" && !editable[at + op])) return false;
    const control = this.host.control();
    if (!control?.enabled()) { this.host.notice(OFF, "warning"); return false; }
    const next = editable[at + 1]?.key ?? editable[at - 1]?.key;
    this.pending = key; this.host.changed();
    let result: string | boolean | undefined;
    try {
      result = op === "edit" ? await control.edit(key) : op === "remove" ? await control.remove(key) : await control.move(key, op);
    } catch {
      result = undefined;
    } finally {
      this.pending = undefined;
    }
    if (typeof result === "string") {
      // The draft is read after the withdrawal settles, so typing during the call is kept.
      this.host.setDraft([result, this.host.draft()].filter(part => part.trim()).join("\n\n"));
      this.focused = false;
      this.host.focusComposer();
    } else if (result === true && op === "remove") {
      if (this.selected === key) this.selected = next;
      this.host.notice("Removed queued input · not sent", "text");
    } else if (result === true) {
      this.selected = key;
    } else {
      this.host.notice(control.enabled() ? GONE : OFF, "warning");
    }
    this.host.changed();
    return result !== undefined && result !== false;
  }

  private header(total: number, hidden: number, tight: boolean, width: number): string {
    const inner = Math.max(0, width - 4);
    const more = hidden > 0 && `+${hidden} more`.length + 2 < inner ? `+${hidden} more` : "";
    const lead = tight ? `QUEUE · ${total}` : `QUEUE · ${total} waiting · each runs when the agent is next idle`;
    return "  " + PANEL_MUTED + fit(lead, inner - (more ? more.length + 2 : 0)) + (more ? "  " + more : "") + "  " + RESET;
  }

  /** `▌ n  steer  text … edit  ↑  ↓  ×`; hit ranges are measured while the row is drawn. */
  private row(item: QueuedItem, at: number, count: number, tight: boolean, width: number, line: number): string {
    const selected = this.focused && item.key === this.selected;
    const indexCells = tight ? 2 : 3, gap = tight ? 1 : 2;
    const parts: readonly { readonly glyph: string; readonly op: QueueOp; readonly enabled: boolean; readonly color: string }[] = [
      { glyph: tight ? "✎" : "edit", op: "edit", enabled: true, color: PANEL_BLUE },
      { glyph: "↑", op: -1, enabled: at > 0, color: at > 0 ? PANEL_TEXT : PANEL_MUTED },
      { glyph: "↓", op: 1, enabled: at < count - 1, color: at < count - 1 ? PANEL_TEXT : PANEL_MUTED },
      { glyph: "×", op: "remove", enabled: true, color: PANEL_ERROR },
    ];
    const controlCells = parts.reduce((sum, part) => sum + visibleWidth(part.glyph), 0) + gap * (parts.length - 1);
    const lead = 2 + indexCells + POLICY_CELLS;
    // Controls stay only while the text keeps a few readable cells.
    const controls = this.controllable && width - lead - 2 - controlCells - gap >= 4;
    const textCells = Math.max(0, width - lead - 2 - (controls ? controlCells + gap : 0));
    const hits: QueueHit[] = [{ from: 0, to: lead + textCells, key: item.key, op: "select" }];
    let out = (selected ? PANEL_SELECTED : "") + " " + PANEL_BLUE + (selected ? "▌" : " ")
      + PANEL_MUTED + fit(String(at + 1), indexCells)
      + (item.policy === "steer" ? PANEL_BLUE + fit("steer", POLICY_CELLS) : PANEL_MUTED + fit("next", POLICY_CELLS))
      + PANEL_TEXT + fit(plainLabel(item.text), textCells);
    if (controls && this.pending === item.key) out += PANEL_MUTED + " ".repeat(gap + controlCells - 1) + "…";
    else if (controls) {
      let x = lead + textCells + gap;
      out += " ".repeat(gap) + parts.map(part => {
        const cells = visibleWidth(part.glyph);
        if (part.enabled) hits.push({ from: x, to: x + cells, key: item.key, op: part.op });
        x += cells + gap;
        return part.color + part.glyph;
      }).join(" ".repeat(gap));
    }
    this.hits.set(line, hits);
    return out + "  " + RESET;
  }
}
