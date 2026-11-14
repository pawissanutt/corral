// Row parts shared by the owner card's list forms for Pi's pickers and settings: one item is a row of columns, and
// the selected item adds the rows that say in full whatever its row had to clip.
import { stripTerminalSequences, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

/** Blank cells between two columns. */
export const GAP = "  ";
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

/** Caller text on one line: terminal sequences removed, controls as blanks; leading blanks (Pi's mark column) kept. */
export function rowText(text: string): string {
  return stripTerminalSequences(text).replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").trimEnd();
}

/** `text` clipped with `…` to `width` cells and padded to exactly `width`; bold for the selected row. */
export function cell(text: string, width: number, bold = false): string {
  const room = Math.max(0, width);
  const clipped = visibleWidth(text) > room ? truncateToWidth(text, room, "…") : text;
  return (bold ? BOLD + clipped + NORMAL : clipped) + " ".repeat(Math.max(0, room - visibleWidth(clipped)));
}

/** `text` wrapped whole into rows of `width` cells, `indent` blanks in, in `color` (muted by default). */
export function detailRows(text: string, width: number, indent = 2, color = PANEL_MUTED): string[] {
  if (!text) return [];
  const room = Math.max(1, width - indent);
  return wrapTextWithAnsi(text, room).map(row => " ".repeat(indent) + color + row + PANEL_TEXT);
}
