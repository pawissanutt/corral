// Compact paging over inspector rows: one item model for the rail, the inline inspector and both
// input routes, so a keyboard step and a pager click always land on the same item.
import type { PanelLine } from "./panels-view.ts";

export interface PageWindow {
  readonly start: number;
  readonly rows: readonly PanelLine[];
  /** Zero-based first visible item: the position a scroll list keeps across relocation. */
  readonly offset: number;
  /** One-based visible item range and the item count, for the pager label. */
  readonly first: number;
  readonly last: number;
  readonly total: number;
}

const clamp = (value: number, low: number, high: number) => Math.max(low, Math.min(high, value));
const isItem = (row: PanelLine) => !row.heading && row.text.trim() !== "";

function finish(all: readonly PanelLine[], items: readonly number[], start: number, capacity: number): PageWindow {
  const end = Math.min(all.length, start + Math.max(0, capacity));
  const visible = items.filter(row => row >= start && row < end);
  const first = visible.length ? items.indexOf(visible[0] ?? -1) + 1 : 0;
  const last = visible.length ? items.indexOf(visible.at(-1) ?? -1) + 1 : 0;
  return { start, rows: all.slice(start, end), offset: Math.max(0, first - 1), first, last, total: items.length };
}

/** Scroll lists keep `offset` as an item index, so layouts with and without headings agree. */
export function scrollWindow(all: readonly PanelLine[], capacity: number, offset: number): PageWindow {
  const items = all.flatMap((row, index) => isItem(row) ? [index] : []);
  const row = items[clamp(offset, 0, items.length - 1)] ?? 0;
  const start = clamp(row - (all[row - 1]?.heading ? 1 : 0), 0, Math.max(0, all.length - capacity));
  return finish(all, items, start, capacity);
}

/** Selection lists keep the selected row visible with the least movement from the previous start. */
export function selectionWindow(all: readonly PanelLine[], capacity: number, selected: number, previous: number): PageWindow {
  const paths = new Set<string>();
  const items = all.flatMap((row, index) => {
    if (row.action?.kind !== "diff" || paths.has(row.action.path)) return [];
    paths.add(row.action.path); return [index];
  });
  let start = clamp(previous, 0, Math.max(0, all.length - capacity));
  if (selected >= 0) start = clamp(Math.max(Math.min(start, selected), selected - capacity + 1), 0, Math.max(0, all.length - capacity));
  const window = finish(all, items, start, capacity);
  const position = items.indexOf(selected);
  return position < 0 ? window : { ...window, offset: position, first: position + 1, last: position + 1 };
}

export function pagerLabel(window: PageWindow, rowCount: number): string {
  if (window.total < 2 || rowCount <= window.rows.length || window.first === 0) return "";
  return `‹ ${window.first}${window.last > window.first ? "–" + window.last : ""}/${window.total} ›`;
}
