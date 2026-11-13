// The docked choice dialog above the composer: an accent card with the question and the policy note in its
// header, an optional body and numbered options. One dialog at a time; the controller decides which one and
// what picking an option does.
import { decodeKittyPrintable, matchesKey, stripTerminalSequences, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent } from "@earendil-works/pi-tui";
import { card as frameCard } from "../frame.ts";
import { plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "../panels-view.ts";

export interface ChoiceOption { readonly label: string; readonly detail?: string; readonly unavailable?: string }
export interface ChoiceDialog {
  /** "exit:<n>" | "question:<child>:<askedAt>" | "plan:<n>": selection resets when the key changes. */
  readonly key: string;
  /** `short` replaces `title` below 70 columns. `titleColor` colours the title's first word (an agent name). */
  readonly title: string; readonly short?: string; readonly titleColor?: string;
  /** The policy note, muted at the right of the header from 96 columns. */
  readonly meta?: string;
  /** Question text or preview table, drawn into at most `rows` rows. */
  readonly body?: (width: number, rows: number) => string[];
  readonly options: readonly ChoiceOption[];
  /** Labels that are sentences: every option takes the two-row form (label row, detail under it) at any width. */
  readonly stacked?: boolean;
}

/** From this many columns each option takes one row: `n label(26) detail`. */
const ONE_ROW = 84;
const META = 96;
const SHORT_TITLE = 70;
const LABEL = 26;
/** The policy note needs this many cells beside the question, else it is dropped. */
const META_MIN = 8;
/** Rows of a wrapped detail below the narrow tier's label row. */
const DETAIL_ROWS = 3;
/** Enter and digits wait this long after a dialog opened by itself. */
export const CHOICE_GUARD_MS = 400;
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

/** `min(max(4, floor(rows × 0.58)), rows − chromeRows − 2)`. */
export function choiceRows(terminalRows: number, chromeRows: number): number {
  return Math.max(0, Math.min(Math.max(4, Math.floor(terminalRows * 0.58)), terminalRows - chromeRows - 2));
}

/** Plain `text` clipped to `width` cells with an ellipsis, then padded. */
function fit(text: string, width: number): string {
  if (width <= 0) return "";
  const out = visibleWidth(text) > width ? truncateToWidth(text, width - 1, "").replaceAll("\x1b[0m", "") + "…" : text;
  return out + " ".repeat(Math.max(0, width - visibleWidth(out)));
}

/** The header: the question in bold (its first word in `titleColor` when set) and the policy note muted at the right. */
function header(dialog: ChoiceDialog, width: number, joined: string | undefined): { readonly left: string; readonly right?: string } {
  const inner = width - 4;
  const base = plainLabel(width < SHORT_TITLE && dialog.short ? dialog.short : dialog.title);
  const title = joined ? `${base}: ${joined}` : base;
  const shown = visibleWidth(title) > inner ? fit(title, inner) : title;
  const space = shown.indexOf(" ");
  const left = BOLD + (dialog.titleColor && space > 0
    ? dialog.titleColor + shown.slice(0, space) + PANEL_TEXT + shown.slice(space)
    : PANEL_TEXT + shown) + NORMAL;
  // The question keeps its cells first; the note takes what is left after a two-cell gap, with an ellipsis.
  const meta = width >= META && dialog.meta ? plainLabel(dialog.meta) : "";
  const room = inner - visibleWidth(shown) - 2;
  if (!meta || room < META_MIN) return { left };
  return { left, right: PANEL_MUTED + (visibleWidth(meta) > room ? fit(meta, room) : meta) };
}

/**
 * The body rows of one option. From 84 columns one row: `n`, the label in a 26-cell column, the detail.
 * Below, the label row and the detail wrapped under the label; `short` drops the detail.
 * An unavailable option is muted and shows its reason where the detail would be.
 */
function optionRows(option: ChoiceOption, index: number, inner: number, one: boolean, short: boolean): string[] {
  const off = option.unavailable !== undefined;
  const number = (off ? PANEL_MUTED : PANEL_BLUE) + fit(String(index + 1), 2);
  const label = plainLabel(option.label);
  const labelStyle = off ? PANEL_MUTED : PANEL_TEXT + BOLD;
  const reason = off ? "unavailable: " + plainLabel(option.unavailable ?? "") : "";
  const detail = off ? reason : option.detail ? plainLabel(option.detail) : "";
  const detailStyle = off ? PANEL_WARNING : PANEL_MUTED;
  const room = Math.max(0, inner - 2);
  if (one) {
    return [number + labelStyle + fit(label, LABEL - 1) + NORMAL + " " + detailStyle + fit(detail, Math.max(0, room - LABEL))];
  }
  const suffix = off && short ? " · unavailable" : "";
  const first = number + labelStyle + fit(label, Math.min(Math.max(0, room - visibleWidth(suffix)), visibleWidth(label))) + NORMAL
    + (suffix ? PANEL_WARNING + suffix : "");
  if (short || !detail || room < 1) return [first];
  // The detail wraps under the label; past three rows the third ends in an ellipsis.
  const wrapped = wrapTextWithAnsi(detail, room);
  const kept = wrapped.length > DETAIL_ROWS ? [...wrapped.slice(0, DETAIL_ROWS - 1), fit(wrapped.slice(DETAIL_ROWS - 1).join(" "), room)] : wrapped;
  return [first, ...kept.map(line => "  " + detailStyle + line)];
}

/**
 * The dialog drawn into at most `maxRows` rows of `width` cells: an accent card whose header holds the
 * question and the policy note, then the body (question text or preview table) and the options. When rows
 * are scarce the options come first; the borders stay while the body keeps at least two rows, and give way
 * (the top one first) only below that. The options scroll to keep the selected one in view. With no row left
 * for the body, its first line joins the header. `hits` maps a line to the option drawn on it; `selectedRow`
 * is the first line of the selected option, or -1.
 */
export function choiceLines(dialog: ChoiceDialog, selected: number, width: number, maxRows: number, short: boolean):
  { readonly lines: string[]; readonly hits: ReadonlyMap<number, number>; readonly selectedRow: number } {
  const hits = new Map<number, number>();
  if (width < 8 || maxRows < 1) return { lines: [], hits, selectedRow: -1 };
  const inner = width - 4, one = width >= ONE_ROW && !dialog.stacked;
  const rows: { readonly text: string; readonly option: number }[] = [];
  dialog.options.forEach((option, index) => {
    for (const text of optionRows(option, index, inner, one, short)) rows.push({ text, option: index });
  });
  const budget = maxRows - 1;
  let body: string[] = [], joined: string | undefined, top = false, bottom = false;
  if (rows.length >= budget) {
    if (dialog.body) {
      const first = dialog.body(Math.max(1, width), 1)[0];
      joined = first === undefined ? undefined : plainLabel(stripTerminalSequences(first)) || undefined;
    }
  } else {
    const room = budget - rows.length;
    let borders = Math.min(2, room);
    if (dialog.body) {
      const natural = dialog.body(inner, room).slice(0, room);
      borders = room >= 4 ? 2 : Math.max(0, Math.min(2, room - natural.length));
      body = borders && natural.length > room - borders ? dialog.body(inner, room - borders).slice(0, room - borders) : natural;
    }
    bottom = borders >= 1; top = borders >= 2;
  }
  const first = rows.findIndex(row => row.option === selected);
  let last = first;
  while (last >= 0 && rows[last + 1]?.option === selected) last++;
  const shown = Math.min(rows.length, budget);
  // The window keeps the selected option's first row in view, and as many of its rows as fit.
  const offset = first < 0 ? 0 : Math.max(0, Math.min(first, last - shown + 1));
  const window = rows.slice(offset, offset + shown);
  const from = window.findIndex(row => row.option === selected);
  let to = from;
  while (from >= 0 && window[to + 1]?.option === selected) to++;
  const head = header(dialog, width, joined);
  const card = frameCard(head.left, [...body, ...window.map(row => row.text)], width, {
    tone: "accent", ...(head.right ? { right: head.right } : {}),
    ...(from >= 0 ? { selected: { from: body.length + from, to: body.length + to } } : {}),
  });
  const lines = card.slice(top ? 0 : 1, bottom ? card.length : card.length - 1);
  const start = lines.length - (bottom ? 1 : 0) - window.length;
  window.forEach((row, at) => hits.set(start + at, row.option));
  return { lines, hits, selectedRow: from >= 0 ? start + from : -1 };
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
