// Semantic selection. On a mouse press pi-tui's alternate-screen renderer asks the component under the pointer for
// the rows of the one content block there (`Component.selectionRows(width, y)`, in the component's own coordinates)
// and then selects and copies only their text: glyphs, indents, gutters, labels and padding stay out, and the rows of
// one wrapped line join without a line break. A component that offers none keeps the plain screen selection.
import { selectionPieces, stripTerminalSequences, wrapTextWithAnsi, type Component, type SelectionRow } from "@earendil-works/pi-tui";

export type { SelectionRow };

/** `rows` moved `down` rows and `right` columns. */
export function shiftRows(rows: readonly SelectionRow[] | undefined, down: number, right: number): readonly SelectionRow[] | undefined {
  return rows?.map(row => ({ ...row, y: row.y + down, left: row.left + right }));
}
/** The rows `component` offers at local row `y` when drawn `width` cells wide from row `top` and column `left`. */
export function selectionIn(component: Component, width: number, y: number, top: number, left: number): readonly SelectionRow[] | undefined {
  return y < top ? undefined : shiftRows(component.selectionRows?.(width, y - top), top, left);
}

/** One drawn row of a block: the plain text on screen, what selecting it copies, and its line. */
export interface DrawnRow {
  readonly text: string;
  readonly copy: string;
  readonly line: number;
  /** Cells before the copied text, such as the `…` of a cut. */
  readonly lead?: number;
}
/**
 * Plain `source` wrapped to `width` as `wrapTextWithAnsi` wraps it: each source line ("\n") gives its rows, numbered
 * from `line`, and a row copies its text plus the whitespace the wrap dropped after it, so a line's rows join back
 * to it.
 */
export function wrappedRows(source: string, width: number, line = 0): DrawnRow[] {
  const rows = wrapTextWithAnsi(source, width).map(row => stripTerminalSequences(row));
  const pieces = selectionPieces(source, rows);
  return rows.map((text, index) => {
    const piece = pieces?.[index];
    return piece ? { text, copy: piece.text, line: line + piece.line } : { text, copy: text, line: line + index };
  });
}
/** `rows` as one block drawn from local row `top` and column `left`. Blank rows stay in, so a copy keeps its blank lines. */
export function blockRows(rows: readonly DrawnRow[], top: number, left: number): SelectionRow[] {
  return rows.map((row, index) => ({ y: top + index, left: left + (row.lead ?? 0), logicalLine: row.line, text: row.copy }));
}
