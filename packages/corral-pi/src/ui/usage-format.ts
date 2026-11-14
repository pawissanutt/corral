// Number and row formatting shared by the Usage page's sections (usage-view.ts, usage-budget-view.ts). Exact values:
// integers grouped, USD at the stored six decimals, an absent value `unknown`, never zero.
import { visibleWidth } from "@earendil-works/pi-tui";
import { wrap } from "./panel-parts.ts";
import { pad, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

/** Below this view width labels, meters and rows take their short forms. */
export const NARROW = 64;

/** Thousands separators on an integer, independent of the host locale. */
export const grouped = (value: number) => String(value).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
/** USD at the stored precision (six decimals), grouped; `unknown` when absent, never zero. */
export function dollars(value: number | null | undefined): string {
  if (value == null) return "unknown";
  const [whole = "0", cents = ""] = value.toFixed(6).split(".");
  return `$${grouped(Number(whole))}.${cents}`;
}
/** Elapsed seconds as `12s`, `1m 04s` or `1h 02m`; `unknown` when not recorded. */
export function wall(seconds: number | null): string {
  if (seconds === null) return "unknown";
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${grouped(Math.floor(seconds / 3600))}h ${String(Math.floor(seconds % 3600 / 60)).padStart(2, "0")}m`;
}
/** `count` grouped with `word`, plural unless the count is one. */
export const plural = (count: number, word: string) => `${grouped(count)} ${word}${count === 1 ? "" : "s"}`;
export const muted = (text: string) => PANEL_MUTED + text + PANEL_TEXT;
/** A muted label column beside a styled value that wraps under itself. */
export function fieldRows(label: string, value: string, labels: number, width: number): string[] {
  return wrap(value, Math.max(1, width - labels - 1)).map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labels)}${PANEL_TEXT} ${row}`);
}
/** `items` joined by ` · ` and broken only between items; an item wider than `width` wraps on its own. */
export function itemRows(items: readonly string[], width: number): string[] {
  const out: string[] = [];
  let line = "";
  for (const item of items) {
    if (line && visibleWidth(line) + 3 + visibleWidth(item) > width) { out.push(line); line = ""; }
    line = line ? `${line} · ${item}` : item;
  }
  if (line) out.push(line);
  return out.flatMap(row => wrap(row, width));
}
