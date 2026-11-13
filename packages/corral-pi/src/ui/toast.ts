// The notice toast at the top right of the main view, for a notice the header has no room for: the raised
// surface, a rounded border in the rule colour, the text in its tone after its glyph. The box is sized from
// the wrapped text, so a long notice takes more rows and is never clipped mid-sentence.
import { visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Notice } from "./header.ts";
import { pad, PANEL_ERROR, PANEL_RAISED, PANEL_RULE, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";

/** Text cells of one toast row at most: a long notice wraps instead of spanning a wide main view. */
const TEXT_MAX = 72;
/** Border and one blank cell each side. */
const FRAME = 4;

/** The glyph that keeps a notice's tone readable without colour: `✓`, `!`, `✗`, none for plain text. */
export function noticeGlyph(tone: Notice["tone"]): string {
  return tone === "success" ? "✓" : tone === "warning" ? "!" : tone === "error" ? "✗" : "";
}

/** The toast's rows and width for `notice` in a main view of `main` columns. */
export function toastFrame(notice: Notice, main: number): { readonly lines: readonly string[]; readonly width: number } {
  const glyph = noticeGlyph(notice.tone) ? noticeGlyph(notice.tone) + " " : "";
  const tone = notice.tone === "success" ? PANEL_SUCCESS : notice.tone === "warning" ? PANEL_WARNING : notice.tone === "error" ? PANEL_ERROR : PANEL_TEXT;
  const room = Math.max(1, Math.min(TEXT_MAX, main - 2 - FRAME));
  const rows = wrapTextWithAnsi(glyph + notice.text, room).map(row => row.trimEnd());
  const text = Math.max(1, ...rows.map(row => visibleWidth(row)));
  const width = text + FRAME;
  const edge = (left: string, right: string) => `${PANEL_RAISED}${PANEL_RULE}${left}${"─".repeat(width - 2)}${right}${RESET}`;
  return {
    width,
    lines: [
      edge("╭", "╮"),
      ...rows.map(row => `${PANEL_RAISED}${PANEL_RULE}│${tone} ${pad(row, text)}${PANEL_RAISED} ${PANEL_RULE}│${RESET}`),
      edge("╰", "╯"),
    ],
  };
}
