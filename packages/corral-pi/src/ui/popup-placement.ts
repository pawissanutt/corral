// Where an overlay popup sits on the terminal: the F1 keys list, the F2 actions, the History list and the resume list.
import type { OverlayOptions } from "@earendil-works/pi-tui";
import { SHORT_ROWS } from "./header.ts";

/** Centred, at most 78 cells wide, 12% down (one row down when short), never taller than the rows below it; 80×24 without a terminal. */
export function popupPlacement(terminal: { readonly columns: number; readonly rows: number } | undefined): OverlayOptions {
  const columns = terminal?.columns ?? 80, rows = terminal?.rows ?? 24;
  const top = rows < SHORT_ROWS ? 1 : Math.round(rows * 0.12);
  return { anchor: "top-center", width: Math.max(20, Math.min(78, columns - 4)), offsetY: top, maxHeight: Math.max(4, rows - top - 1) };
}
