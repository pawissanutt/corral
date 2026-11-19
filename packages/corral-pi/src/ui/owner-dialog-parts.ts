// The owner card's pure parts: the hint row every dialog draws under its card, caller text as display lines,
// option rows, the scrolled body window, and the spec a card is built from. No state, no I/O.
import { stripTerminalSequences, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { keyCap } from "./frame.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, plainLabel } from "./panels-view.ts";

/** One key of a dialog's hint row: the cap's text, what it does, and the input a click on it replays. */
export interface DialogHint { readonly key: string; readonly label: string; readonly data: string }
/** A drawn cap's cells `[from, to)` on the hint row and the input a click there replays through `handleInput`. */
export interface DialogHintHit { readonly from: number; readonly to: number; readonly data: string }

/**
 * A dialog's one hint row, exactly `width` cells: a cap and a muted label per key, from the second cell (under the
 * card's left border), two cells apart, in priority order; a key that no longer fits ends the row, so the first
 * ones always show. Every drawn cap is a hit.
 */
export function dialogHintRow(keys: readonly DialogHint[], width: number): { readonly line: string; readonly hits: readonly DialogHintHit[] } {
  const room = Math.max(0, width);
  if (room === 0) return { line: "", hits: [] };
  const hits: DialogHintHit[] = [];
  let line = " ", x = 1;
  for (const item of keys) {
    const key = plainLabel(item.key), label = plainLabel(item.label);
    const size = visibleWidth(key) + 2 + (label ? 1 + visibleWidth(label) : 0);
    const from = hits.length ? x + 2 : x;
    // One blank cell stays at the right edge, as under the card.
    if (from + size > room - 1) break;
    line += (hits.length ? "  " : "") + keyCap(key) + (label ? " " + PANEL_MUTED + label + PANEL_TEXT : "");
    hits.push({ from, to: from + size, data: item.data });
    x = from + size;
  }
  return { line: PANEL_BG + PANEL_TEXT + line + " ".repeat(Math.max(0, room - visibleWidth(line))), hits };
}

export const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

export const muted = (text: string) => PANEL_MUTED + text + PANEL_TEXT;
const plural = (count: number, noun: string) => `${count} ${noun}${count === 1 ? "" : "s"}`;

/** Caller text as display lines: terminal sequences removed, tabs as two blanks, other controls as one. */
export function textLines(text: string): string[] {
  return stripTerminalSequences(text).replace(/\r\n?/g, "\n").split("\n")
    .map(line => line.replace(/\t/g, "  ").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").trimEnd());
}
export function trimBlank(lines: readonly string[]): string[] {
  let from = 0, to = lines.length;
  while (from < to && !lines[from]!.trim()) from++;
  while (to > from && !lines[to - 1]!.trim()) to--;
  return lines.slice(from, to);
}
/** Seconds as `m:ss`, rounded up. */
export function clock(seconds: number): string {
  const total = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/**
 * An option of a Pi dialog: its exact label, numbered when digits pick it. A neutral row when not selected (muted
 * number, plain label, clipped); the selected row bold with an accent number, wrapped in full.
 */
export function optionLines(label: string, number: number | undefined, width: number, selected: boolean): string[] {
  const text = plainLabel(label);
  const indent = number === undefined ? 0 : 3;
  const lead = number === undefined ? "" : (selected ? PANEL_BLUE : PANEL_MUTED) + String(number).padEnd(indent) + PANEL_TEXT;
  const room = Math.max(1, width - indent);
  if (!selected) return [lead + (visibleWidth(text) > room ? truncateToWidth(text, room, "…") : text)];
  const wrapped = wrapTextWithAnsi(text, room);
  return (wrapped.length ? wrapped : [""]).map((line, at) => (at ? " ".repeat(indent) : lead) + BOLD + line + NORMAL);
}

/**
 * The body text in `rows` rows from line `offset`. Overflow is said in place: `… n lines above` and `… n more lines`
 * from three rows; with one row and `summary`, only the line count (the whole text is one key away).
 */
export function bodyView(lines: readonly string[], rows: number, offset: number, summary: boolean):
  { readonly rows: string[]; readonly offset: number; readonly max: number } {
  const total = lines.length;
  if (rows <= 0 || !total) return { rows: [], offset: 0, max: 0 };
  if (total <= rows) return { rows: [...lines], offset: 0, max: 0 };
  if (rows === 1 && summary) return { rows: [muted(`… ${plural(total, "line")}`)], offset: 0, max: 0 };
  const marks = rows >= 3;
  const max = total - (marks ? rows - 1 : rows);
  const start = Math.max(0, Math.min(offset, max));
  const top = marks && start > 0;
  let room = rows - (top ? 1 : 0);
  const bottom = marks && start + room < total;
  if (bottom) room--;
  return {
    rows: [...(top ? [muted(`… ${plural(start, "line")} above`)] : []), ...lines.slice(start, start + room),
      ...(bottom ? [muted(`… ${total - start - room} more ${total - start - room === 1 ? "line" : "lines"}`)] : [])],
    offset: start, max,
  };
}

/** `value`: what the selection follows when `entries()` returns new rows (a refreshed list), compared with `===`. */
export interface Entry { readonly match: string; readonly value?: unknown; readonly disabled?: string | undefined; lines(width: number, selected: boolean): readonly string[] }

/** What one card shows and does; built from a Pi request or from `ownerListCard` options. */
export interface CardSpec {
  readonly question: string;
  readonly body: readonly string[];
  /** Lines after `body`, read at each render (a picker's refresh status); drawn and fitted as the body. */
  more?(): readonly string[];
  /** A body cut to one row shows its first row (paged by the page keys), never only its line count. */
  readonly lead?: boolean;
  /** `choice`: numbered options, digits pick. `list`: a filter row over a scrolling list. `input`: one text row. */
  readonly form: "choice" | "list" | "input";
  readonly placeholder: string;
  /** `list` only: the filter's text when the card opens. */
  readonly filter?: string;
  /** `list` only: the entry selected when the card opens. */
  initial?(entries: readonly Entry[]): number;
  /** Seconds left when Pi runs a timeout, until Pi's first `setCountdown`. */
  readonly seconds: number | undefined;
  readonly enter: string;
  /** What Esc does, as the hint names it. */
  readonly escape: string;
  /** Enter, digits and picking clicks wait CHOICE_GUARD_MS after the card opened. */
  readonly guard: boolean;
  /** Rows the card may draw, its hint row included. */
  budget(): number;
  /** The card changed outside a delivered key (a countdown tick, a click): redraw. */
  changed(): void;
  entries(): readonly Entry[];
  right?(): string;
  expandTools?(): void;
  key?(data: string, index: number | undefined, filter: string): boolean;
  hints?(index: number | undefined): readonly DialogHint[];
  /** True when the entry acted in place and the card stays open. */
  act?(index: number): boolean;
  moved?(index: number | undefined): void;
  choose(index: number): void;
  submit(text: string): void;
  cancel(): void;
  /** Pi removed the card (an answer, its timeout, an abort); called once. */
  closed?(): void;
}
