// Shared drawing vocabulary for the read-only inspection views (Proof, Inbox): chips, section rules,
// list rows with the selection bar, label/value fields, short times and the pinned key-hint row.
// Colours are the packaged PANEL_* tokens; colour is never the only signal.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_TINT,
  PANEL_WARNING, RESET,
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
export function sectionRule(label: string, width: number): string {
  const head = plainLabel(label).toUpperCase();
  return `${PANEL_MUTED}${bold(head)} ${PANEL_RULE}${"─".repeat(Math.max(0, width - visibleWidth(head) - 1))}${PANEL_TEXT}`;
}
/** A muted label column beside wrapped text; an empty value reads as `—`. */
export function field(label: string, value: string, width: number, labelWidth: number): string[] {
  return wrap(plainLabel(value) || "—", width - labelWidth - 1)
    .map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labelWidth)}${PANEL_TEXT} ${row}`);
}
/** One list-pane row `width` wide; the selected row carries the blue `▌` bar on the selection surface. */
export function listRow(left: string, right: string, selected: boolean, width: number): string {
  const body = splitRow(left, right, width - 3);
  return selected ? `${PANEL_SELECTED}${PANEL_BLUE}▌${PANEL_TEXT} ${body} ${BG_END}` : `  ${body} `;
}
/** The pinned bottom hint row: full labels, then brief labels, then bare keys, so every key stays listed. */
export function hintRow(hints: readonly (readonly [key: string, full: string, brief: string])[], width: number): string {
  const forms = [hints.map(([key, full]) => `${key} ${full}`).join(" · "), hints.map(([key, , brief]) => `${key} ${brief}`).join(" · "),
    hints.map(([key]) => key).join(" ")];
  const text = forms.find(form => visibleWidth(form) + 2 <= width) ?? forms[forms.length - 1] ?? "";
  return `${PANEL_BG}${PANEL_MUTED}${pad("  " + text, width)}${RESET}`;
}

export function ago(at: number, now: number): string | undefined {
  if (!Number.isFinite(at)) return undefined;
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  return seconds < 60 ? `${seconds}s ago` : seconds < 3600 ? `${Math.floor(seconds / 60)}m ago`
    : seconds < 86_400 ? `${Math.floor(seconds / 3600)}h ago` : `${Math.floor(seconds / 86_400)}d ago`;
}
export function clock(at: number): string {
  const d = new Date(at);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
