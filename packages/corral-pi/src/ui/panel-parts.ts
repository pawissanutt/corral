// Shared drawing vocabulary for the read-only inspection views (Proof, Inbox, Graph, Timeline): chips, the chip row
// with its freshness note, section rules, list rows with the selection bar, aligned list columns, label/value fields
// and short times. Key hints live in the footer.
// Colours are the packaged PANEL_* tokens; colour is never the only signal.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_LINE, PANEL_MUTED, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_TINT,
  PANEL_WARNING,
} from "./panels-view.ts";

export type Tone = "ok" | "bad" | "warn" | "none";
export const toneColor = (t: Tone) => t === "ok" ? PANEL_SUCCESS : t === "bad" ? PANEL_ERROR : t === "warn" ? PANEL_WARNING : PANEL_MUTED;
export const bold = (text: string) => `\x1b[1m${text}\x1b[22m`;
/** Ends a chip or selection background; the panel canvas maps the default back to its own surface. */
export const BG_END = "\x1b[49m";
export const wrap = (text: string, width: number) => wrapTextWithAnsi(text, Math.max(1, width));

/** A chip: glyph and word on a tone-tinted surface, coloured by tone. */
export function chip(text: string, tone: Tone = "none"): string {
  const back = tone === "ok" ? PANEL_TINT.success : tone === "bad" ? PANEL_TINT.error : tone === "warn" ? PANEL_TINT.warning : PANEL_TINT.raised;
  return `${back}${tone === "none" ? PANEL_TEXT : toneColor(tone)} ${plainLabel(text)} ${BG_END}${PANEL_TEXT}`;
}
export function chipRows(chips: readonly string[], width: number): string[] {
  const rows: string[] = [];
  let row = "";
  for (const c of chips) {
    const piece = visibleWidth(c) > width ? truncateToWidth(c, width, "…") : c;
    if (row && visibleWidth(row) + 1 + visibleWidth(piece) > width) { rows.push(row); row = ""; }
    row = row ? `${row} ${piece}` : piece;
  }
  if (row) rows.push(row);
  return rows;
}
/** A section heading in muted bold followed by a rule in the line colour to `width`. */
export function sectionRule(label: string, width: number): string {
  const head = plainLabel(label).toUpperCase();
  return `${PANEL_MUTED}${bold(head)} ${PANEL_LINE}${"─".repeat(Math.max(0, width - visibleWidth(head) - 1))}${PANEL_TEXT}`;
}
/** A muted label column beside wrapped text; an empty value reads as `—`. */
export function field(label: string, value: string, width: number, labelWidth: number): string[] {
  return wrap(plainLabel(value) || "—", width - labelWidth - 1)
    .map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labelWidth)}${PANEL_TEXT} ${row}`);
}
/** One list-pane row `width` wide; the selected row carries the `▌` bar on the selection surface, accent while `focused`, else muted. */
export function listRow(left: string, right: string, selected: boolean, width: number, focused = true): string {
  const body = splitRow(left, right, width - 3);
  return selected ? `${PANEL_SELECTED}${focused ? PANEL_BLUE : PANEL_MUTED}▌${PANEL_TEXT} ${body} ${BG_END}` : `  ${body} `;
}
/**
 * A sub-view's back line on row 0: `‹ Parent` in accent, ` · ` muted, `title` bold, `right` muted at the right edge
 * (dropped first when narrow). `to` ends the hit range of `‹ Parent`, which starts at column 0.
 */
export function backLine(parent: string, title: string, right: string, width: number): { line: string; to: number } {
  const back = `‹ ${plainLabel(parent)}`;
  const left = `${PANEL_BLUE}${back}${PANEL_MUTED} · ${PANEL_TEXT}${bold(plainLabel(title))}${PANEL_TEXT}`;
  const room = width - visibleWidth(left) - 2;
  const tail = right && room >= 6 ? PANEL_MUTED + truncateToWidth(plainLabel(right), room, "…") + PANEL_TEXT : "";
  return { line: tail ? splitRow(left, tail, width) : pad(left, width), to: Math.min(width, visibleWidth(back)) };
}

/** One list column: a muted header label, a width in cells and an alignment. */
export interface Column { readonly label: string; readonly width: number; readonly align?: "right" }
/** A column layout candidate; the `flex` column takes the cells the others leave and needs at least its `width`. */
export interface ColumnLayout { readonly columns: readonly Column[]; readonly flex: number }
/**
 * The first layout whose flex column keeps at least its width inside `inner` cells (columns joined by two blanks),
 * with the flex column widened to fill; the last layout when none fits.
 */
export function fitColumns(layouts: readonly ColumnLayout[], inner: number): Column[] {
  let chosen: Column[] = [];
  for (const layout of layouts) {
    const fixed = layout.columns.reduce((sum, c, i) => sum + (i === layout.flex ? 0 : c.width), 0) + 2 * Math.max(0, layout.columns.length - 1);
    const room = inner - fixed, flex = layout.columns[layout.flex];
    chosen = layout.columns.map((c, i) => i === layout.flex ? { ...c, width: Math.max(1, room) } : c);
    if (flex && room >= flex.width) break;
  }
  return chosen;
}
/** `values` set into `columns`: each clipped with `…` and padded (right-aligned where asked), two blanks apart. */
export function cells(columns: readonly Column[], values: readonly string[]): string {
  return columns.map((c, i) => {
    const value = values[i] ?? "", fitted = pad(value, c.width);
    if (c.align !== "right") return fitted;
    const text = truncateToWidth(value, c.width, "…");
    return " ".repeat(Math.max(0, c.width - visibleWidth(text))) + text;
  }).join("  ");
}
/** The muted header row of a list, aligned with `listRow` rows of the same `columns` and `width`. */
export function headerRow(columns: readonly Column[], width: number): string {
  return listRow(PANEL_MUTED + cells(columns, columns.map(c => c.label)) + PANEL_TEXT, "", false, width);
}
/**
 * The chip rows of a view with its freshness note right-aligned on the last row: `read 12:04 (2m ago) · r reread`,
 * shortening to `read 2m ago · r reread`, then `r reread`, which takes its own row when nothing else fits.
 * `at` undefined reads `not read`; `note` replaces the read note for sources that are not reread (`r reread` dropped).
 */
export function chipHeader(chips: readonly string[], width: number, at: number | undefined, now: number, note?: string): string[] {
  const since = at === undefined ? undefined : ago(at, now);
  const reads = note !== undefined ? [note] : at !== undefined && since
    ? [`read ${clock(at)} (${since}) · r reread`, `read ${since} · r reread`, "r reread"] : ["not read · r reread", "r reread"];
  const rows = chipRows(chips, width), last = rows.at(-1) ?? "";
  const read = reads.find(r => visibleWidth(last) + 2 + visibleWidth(r) <= width);
  const out = [...rows.slice(0, -1), read ? splitRow(last, PANEL_MUTED + read + PANEL_TEXT, width) : last];
  const fallback = reads.at(-1) ?? "";
  if (!read && fallback) out.push(pad(PANEL_MUTED + fallback + PANEL_TEXT, width));
  return out;
}

export function ago(at: number, now: number): string | undefined {
  if (!Number.isFinite(at)) return undefined;
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  return seconds < 60 ? `${seconds}s ago` : seconds < 3600 ? `${Math.floor(seconds / 60)}m ago`
    : seconds < 86_400 ? `${Math.floor(seconds / 3600)}h ago` : `${Math.floor(seconds / 86_400)}d ago`;
}
/** `HH:MM` local time, or `HH:MM:SS` with `seconds`. */
export function clock(at: number, seconds = false): string {
  const d = new Date(at), two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}${seconds ? `:${two(d.getSeconds())}` : ""}`;
}
