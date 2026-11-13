// The docked choice dialog above the composer: a title row, an optional body and numbered options.
// One dialog at a time; the controller decides which one and what picking an option does.
import { decodeKittyPrintable, matchesKey, stripTerminalSequences, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent } from "@earendil-works/pi-tui";
import { plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_SELECTED, PANEL_TEXT, PANEL_WARNING, RESET } from "../panels-view.ts";

export interface ChoiceOption { readonly label: string; readonly detail?: string; readonly unavailable?: string }
export interface ChoiceDialog {
  /** "exit:<n>" | "question:<child>:<askedAt>" | "plan:<n>": selection resets when the key changes. */
  readonly key: string;
  /** `short` replaces `title` below 70 columns. `titleColor` colours the title's first word (an agent name). */
  readonly title: string; readonly short?: string; readonly titleColor?: string;
  /** Drawn at the right of the title row from 96 columns. */
  readonly meta?: string;
  /** Question text or preview table, drawn into at most `rows` rows. */
  readonly body?: (width: number, rows: number) => string[];
  readonly options: readonly ChoiceOption[];
}

/** From this many columns each option takes one row: `▌ n label(26) detail`. */
const ONE_ROW = 84;
const META = 96;
const SHORT_TITLE = 70;
const LABEL = 26;
/** Enter and digits wait this long after a dialog opened by itself. */
export const CHOICE_GUARD_MS = 400;
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

/** `min(max(4, floor(rows × 0.58)), rows − chromeRows − 2)`. */
export function choiceRows(terminalRows: number, chromeRows: number): number {
  return Math.max(0, Math.min(Math.max(4, Math.floor(terminalRows * 0.58)), terminalRows - chromeRows - 2));
}

/** `text` clipped to `width` cells with an ellipsis, then padded; reset codes are removed so a row background survives. */
function fit(text: string, width: number): string {
  if (width <= 0) return "";
  const out = visibleWidth(text) > width ? truncateToWidth(text, width - 1, "").replaceAll(RESET, "") + "…" : text;
  return out + " ".repeat(Math.max(0, width - visibleWidth(out)));
}

/** One inner row between the side borders; `back` fills the row, padding included. */
function framed(content: string, inner: number, back = ""): string {
  return PANEL_BLUE + "│" + back + " " + fit(content, inner) + back + " " + RESET + PANEL_BLUE + "│" + RESET;
}

function titleRow(dialog: ChoiceDialog, width: number, joined: string | undefined): string {
  const base = plainLabel(width < SHORT_TITLE && dialog.short ? dialog.short : dialog.title);
  const title = joined ? `${base}: ${joined}` : base;
  const meta = width >= META && dialog.meta ? plainLabel(dialog.meta) : "";
  // `╭─ ` title ` ` dashes [` meta `] `─╮`, measured from the drawn text: the title keeps its cells first, the
  // meta takes what is left with an ellipsis and is dropped below 8 cells, and the row always ends with `─╮`.
  const head = fit(title, Math.min(visibleWidth(title), Math.max(0, width - 6)));
  const used = 3 + visibleWidth(head) + 1;
  const room = width - used - 1 - 2 - 2;
  const tail = meta && room >= 8 ? fit(meta, Math.min(visibleWidth(meta), room)) : "";
  const dashes = Math.max(0, width - used - 2 - (tail ? visibleWidth(tail) + 2 : 0));
  let shown = head;
  if (dialog.titleColor) {
    const space = shown.indexOf(" ");
    shown = space < 0 ? dialog.titleColor + shown : dialog.titleColor + shown.slice(0, space) + PANEL_TEXT + shown.slice(space);
  } else shown = PANEL_TEXT + shown;
  const metaPart = tail ? " " + PANEL_MUTED + tail + " " + PANEL_BLUE : "";
  return PANEL_BLUE + "╭─ " + BOLD + shown + NORMAL + " " + PANEL_BLUE + "─".repeat(dashes) + metaPart + "─╮" + RESET;
}

/** The rows of one option; two rows below 84 columns unless `short` or there is nothing to say under the label. */
function optionRows(option: ChoiceOption, index: number, selected: boolean, inner: number, one: boolean, short: boolean): string[] {
  const off = option.unavailable !== undefined;
  const bar = selected ? PANEL_BLUE + "▌" : " ";
  const number = (off ? PANEL_MUTED : PANEL_BLUE) + fit(String(index + 1), 2);
  const label = plainLabel(option.label);
  const labelColor = off ? PANEL_MUTED : PANEL_TEXT + BOLD;
  const detail = off ? PANEL_WARNING + "unavailable: " + plainLabel(option.unavailable ?? "")
    : option.detail ? PANEL_MUTED + plainLabel(option.detail) : "";
  if (one) {
    const rest = Math.max(0, inner - 4 - LABEL - 1);
    return [bar + " " + number + labelColor + fit(label, LABEL) + NORMAL + " " + fit(detail, rest)];
  }
  const suffix = off && short ? " · unavailable" : "";
  const room = Math.max(0, inner - 4 - visibleWidth(suffix));
  const first = bar + " " + number + labelColor + fit(label, Math.min(room, visibleWidth(label))) + NORMAL + (suffix ? PANEL_WARNING + suffix : "");
  if (short || !detail) return [first];
  return [first, (selected ? PANEL_BLUE + "▌" : " ") + "   " + fit(detail, Math.max(0, inner - 4))];
}

/**
 * The dialog drawn into at most `maxRows` rows of `width` cells. The title row is pinned; the options
 * come before the body and the bottom border, and scroll to keep the selected one visible when even
 * they do not fit. With no row left for the body, its first line joins the title row. `hits` maps a
 * line to the option drawn on it; `selectedRow` is the first line of the selected option, or -1.
 */
export function choiceLines(dialog: ChoiceDialog, selected: number, width: number, maxRows: number, short: boolean):
  { readonly lines: string[]; readonly hits: ReadonlyMap<number, number>; readonly selectedRow: number } {
  const hits = new Map<number, number>();
  if (width < 8 || maxRows < 1) return { lines: [], hits, selectedRow: -1 };
  const inner = width - 4, one = width >= ONE_ROW;
  const rows: { readonly text: string; readonly option: number }[] = [];
  dialog.options.forEach((option, index) => {
    const on = index === selected;
    for (const text of optionRows(option, index, on, inner, one, short)) {
      rows.push({ text: framed(text, inner, on ? PANEL_SELECTED : ""), option: index });
    }
  });
  const budget = maxRows - 1;
  let body: string[] = [], spacer = false, bottom = false, joined: string | undefined;
  if (rows.length >= budget) {
    if (dialog.body) {
      const first = dialog.body(Math.max(1, width), 1)[0];
      joined = first === undefined ? undefined : plainLabel(stripTerminalSequences(first)) || undefined;
    }
  } else {
    let room = budget - rows.length;
    if (dialog.body) {
      const offered = !short && room >= 2 ? room - 1 : room;
      body = dialog.body(inner, offered).slice(0, offered);
      room -= body.length;
      if (body.length && room > (short ? 0 : 1)) { spacer = true; room--; }
    }
    bottom = !short && room >= 1;
  }
  const first = rows.findIndex(row => row.option === selected);
  let last = first;
  while (last >= 0 && rows[last + 1]?.option === selected) last++;
  const shown = Math.min(rows.length, budget);
  const offset = last >= shown ? Math.min(rows.length - shown, last - shown + 1) : 0;
  const lines = [titleRow(dialog, width, joined)];
  for (const line of body) lines.push(framed(line, inner));
  if (spacer) lines.push(framed("", inner));
  const start = lines.length;
  for (const row of rows.slice(offset, offset + shown)) { hits.set(lines.length, row.option); lines.push(row.text); }
  if (bottom) lines.push(PANEL_BLUE + "╰" + "─".repeat(width - 2) + "╯" + RESET);
  const visible = first >= offset && first < offset + shown;
  return { lines, hits, selectedRow: visible ? start + first - offset : -1 };
}

export interface ChoiceDockHost {
  dialog(): ChoiceDialog | undefined;
  /** Called for available options only. */
  pick(index: number): void;
  /** An unavailable option was picked: the host raises its reason. */
  refuse(option: ChoiceOption): void;
  /** Esc: not now. */
  dismiss(): void;
  /** A plain click the session may act on. */
  click(event: TuiMouseEvent): boolean;
  focus(): void;
  changed(): void;
}

export class ChoiceDock implements Component {
  focused = false;
  selected = 0;
  private key: string | undefined;
  private maxRows = 12;
  private short = false;
  private until = 0;
  private hits: ReadonlyMap<number, number> = new Map();

  constructor(private readonly host: ChoiceDockHost) {}

  setLimit(maxRows: number, short: boolean): void { this.maxRows = maxRows; this.short = short; }

  /** Ignores Enter and digits until `until` (the 400 ms guard after an automatic open). */
  guard(until: number): void { this.until = until; }

  /** ↑↓ j k (wrap), 1-9, Enter, Esc. Other keys are swallowed while focused, except page keys and control sequences. */
  handleKey(data: string): boolean {
    if (!this.focused) return false;
    const dialog = this.current();
    if (!dialog) return false;
    const ch = decodeKittyPrintable(data) ?? data;
    const count = dialog.options.length;
    if (matchesKey(data, "escape")) { this.host.dismiss(); return true; }
    if (matchesKey(data, "up") || ch === "k") { this.step(-1, count); return true; }
    if (matchesKey(data, "down") || ch === "j") { this.step(1, count); return true; }
    if (matchesKey(data, "enter")) { if (Date.now() >= this.until) this.activate(dialog, this.selected); return true; }
    if (/^[1-9]$/.test(ch)) {
      const index = Number(ch) - 1;
      if (Date.now() >= this.until && index < count) this.activate(dialog, index);
      return true;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown") || matchesKey(data, "ctrl+c")) return false;
    // Unbound special keys and terminal replies are not typing; printable keys never reach the composer.
    return !ch.startsWith("\x1b");
  }

  render(width: number): string[] {
    const dialog = this.current();
    if (!dialog) { this.hits = new Map(); return []; }
    const frame = choiceLines(dialog, this.selected, width, this.maxRows, this.short);
    this.hits = frame.hits;
    return frame.lines;
  }

  /** A click selects an option; a click on the selected option picks it. */
  handleMouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    if (!this.host.click(event)) return undefined;
    const dialog = this.current();
    if (!dialog) return undefined;
    this.focused = true;
    this.host.focus();
    const index = this.hits.get(event.y);
    if (index === undefined) { this.host.changed(); return { handled: true }; }
    if (index === this.selected) this.activate(dialog, index);
    else { this.selected = index; this.host.changed(); }
    return { handled: true };
  }

  invalidate(): void {}

  /** The host's dialog; a new key resets the selection to the first option. */
  private current(): ChoiceDialog | undefined {
    const dialog = this.host.dialog();
    if (!dialog) { this.key = undefined; return undefined; }
    if (dialog.key !== this.key) { this.key = dialog.key; this.selected = 0; }
    if (this.selected >= dialog.options.length || this.selected < 0) this.selected = 0;
    return dialog;
  }

  private step(delta: number, count: number): void {
    if (!count) return;
    this.selected = (this.selected + delta + count) % count;
    this.host.changed();
  }

  private activate(dialog: ChoiceDialog, index: number): void {
    const option = dialog.options[index];
    if (!option) return;
    this.selected = index;
    if (option.unavailable !== undefined) this.host.refuse(option);
    else this.host.pick(index);
    this.host.changed();
  }
}
