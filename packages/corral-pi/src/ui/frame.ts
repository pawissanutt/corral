// The frame's shared drawing vocabulary: key caps, count chips, the spinner, rounded cards and titled
// boxes, and rules. Every line is exactly the width it is given and ends on the one canvas background.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import {
  PANEL_BG, PANEL_BLUE, PANEL_DIALOG, PANEL_KEY, PANEL_LINE, PANEL_ON_ACCENT, PANEL_RAISED, PANEL_RULE, PANEL_SELECTED, PANEL_TEXT,
} from "./panels-view.ts";

const SPINNER = "⠋⠙⠹⠸⠼⠴⠦⠧";
/** One spinner frame lasts this long; not a divisor of the one-second redraw tick, so each tick shows a new frame. */
const FRAME_MS = 110;
const RESET = "\x1b[0m";

/**
 * Cells the workbench takes from the Task, Proof, lifecycle, Usage, Artifacts and Inbox views: one blank cell each
 * side, so they sit one cell in like the rest of the frame. Their layout thresholds are main-view widths, so they
 * compare `width + VIEW_INSET`.
 */
export const VIEW_INSET = 2;
/** The canvas style: text colour on the one background. */
function canvas(): string { return PANEL_TEXT + PANEL_BG; }

/**
 * `text` with every return to the canvas (a reset, the default background or the canvas background itself)
 * sent back to `surface` instead, so a styled span keeps a row on its surface.
 */
export function onSurface(text: string, surface: string): string {
  return text.replaceAll(RESET, RESET + PANEL_TEXT + surface).replaceAll("\x1b[49m", surface).replaceAll(PANEL_BG, surface);
}
/** `text` clipped with `…` to `width` cells and padded with blanks on `surface`. */
function fit(text: string, width: number, surface: string): string {
  const room = Math.max(0, width);
  const clipped = room === 0 ? "" : visibleWidth(text) > room ? truncateToWidth(text, room, "…") : text;
  return onSurface(clipped, surface) + RESET + PANEL_TEXT + surface + " ".repeat(Math.max(0, room - visibleWidth(clipped)));
}

/**
 * A key cap: the key in accent on the key surface, one cell of padding each side. `on` draws the active form
 * (accent surface, dark bold text). Ends in the canvas style, or in `restore` (a background) followed by the
 * text colour when the cap sits on another surface.
 */
export function keyCap(key: string, on = false, restore?: string): string {
  const after = (restore ?? PANEL_BG) + PANEL_TEXT;
  return on ? `${PANEL_ON_ACCENT} ${key} \x1b[22m${after}` : `${PANEL_KEY}${PANEL_BLUE} ${key} ${after}`;
}
/** A count chip: text on the selected surface, one cell of padding each side. Ends in the canvas style. */
export function countChip(text: string): string {
  return `${PANEL_SELECTED}${PANEL_TEXT} ${text} ${canvas()}`;
}
/** The braille spinner frame for `now` (milliseconds); `phase` staggers rows. */
export function spinner(now: number, phase = 0): string {
  const step = Math.floor((Number.isFinite(now) ? now : 0) / FRAME_MS) + Math.trunc(Number.isFinite(phase) ? phase : 0);
  return SPINNER[((step % SPINNER.length) + SPINNER.length) % SPINNER.length]!;
}

/** Body rows `[from, to]` (inclusive) on the selected surface, the accent bar on `from`. */
export type CardSelection = number | { readonly from: number; readonly to: number };

/**
 * A rounded card exactly `width` cells wide: top border, one header row on a raised surface (`header` left, `right`
 * right), the body rows, bottom border. Body rows are fitted to `width - 4` and padded; a row that is already styled
 * keeps its style. `tone: "accent"` draws the border in accent and the header on the dialog surface (dialogs); the
 * default draws it in the line colour. `selected` is the body row (or rows) drawn on the selected surface with the
 * accent bar in the first cell of its first row. The header is not bolded here.
 */
export function card(header: string, body: readonly string[], width: number,
  options: { readonly right?: string; readonly tone?: "line" | "accent"; readonly selected?: CardSelection } = {}): string[] {
  if (width < 4) return [];
  const border = options.tone === "accent" ? PANEL_BLUE : PANEL_LINE, surface = options.tone === "accent" ? PANEL_DIALOG : PANEL_RAISED;
  const inner = width - 4, side = PANEL_BG + border + "│";
  const range = options.selected === undefined ? undefined
    : typeof options.selected === "number" ? { from: options.selected, to: options.selected } : options.selected;
  const right = options.right ?? "", rightWidth = right ? Math.min(visibleWidth(right), Math.max(0, inner - 4)) : 0;
  const title = rightWidth ? fit(header, inner - rightWidth - 1, surface) + " " + fit(right, rightWidth, surface) : fit(header, inner, surface);
  const lines = [
    `${PANEL_BG}${border}╭${"─".repeat(width - 2)}╮${canvas()}`,
    `${side}${surface}${PANEL_TEXT} ${title} ${side}${canvas()}`,
  ];
  body.forEach((row, index) => {
    const on = range !== undefined && index >= range.from && index <= range.to;
    const back = on ? PANEL_SELECTED : PANEL_BG;
    const bar = on && index === range.from ? PANEL_BLUE + "▌" + PANEL_TEXT : " ";
    lines.push(`${side}${back}${PANEL_TEXT}${bar}${fit(row, inner, back)} ${side}${canvas()}`);
  });
  lines.push(`${PANEL_BG}${border}╰${"─".repeat(width - 2)}╯${canvas()}`);
  return lines;
}
/**
 * A rounded box with `title` set into its top border (`╭─ title ──╮`); `tone: "accent"` when it has the focus.
 * Every line is exactly `width` cells.
 */
export function titledBox(title: string, body: readonly string[], width: number,
  options: { readonly tone?: "line" | "accent" } = {}): string[] {
  if (width < 4) return [];
  const border = options.tone === "accent" ? PANEL_BLUE : PANEL_LINE, side = PANEL_BG + border + "│";
  const room = width - 6;
  const label = title && room > 0 ? truncateToWidth(title, room, "…") : "";
  const top = label
    ? `${PANEL_BG}${border}╭─ ${PANEL_TEXT}${onSurface(label, PANEL_BG)}${RESET}${PANEL_BG}${border} ${"─".repeat(Math.max(0, width - 5 - visibleWidth(label)))}╮`
    : `${PANEL_BG}${border}╭${"─".repeat(width - 2)}╮`;
  return [
    top + canvas(),
    ...body.map(row => `${side}${PANEL_TEXT} ${fit(row, width - 4, PANEL_BG)} ${side}${canvas()}`),
    `${PANEL_BG}${border}╰${"─".repeat(width - 2)}╯${canvas()}`,
  ];
}
/** A full-width rule of `─` in the line colour. */
export function rule(width: number): string {
  return `${PANEL_BG}${PANEL_LINE}${"─".repeat(Math.max(0, width))}${PANEL_TEXT}`;
}
/**
 * A popup (the F1 keys list, the F2 actions) exactly `width` cells wide on the raised surface: a rounded border in
 * the rule colour, a title row (`title` left, `right` right), a divider in the line colour, then the body rows.
 * Body row `selected` is on the selected surface with the accent bar in its first cell.
 */
export function popup(title: string, right: string, body: readonly string[], width: number, selected?: number): string[] {
  if (width < 6) return [];
  const inner = width - 2, side = PANEL_RAISED + PANEL_RULE + "│";
  const rightWidth = right ? Math.min(visibleWidth(right), Math.max(0, inner - 6)) : 0;
  const head = rightWidth ? fit(title, inner - 3 - rightWidth, PANEL_RAISED) + " " + fit(right, rightWidth, PANEL_RAISED) : fit(title, inner - 2, PANEL_RAISED);
  return [
    `${PANEL_RAISED}${PANEL_RULE}╭${"─".repeat(inner)}╮${canvas()}`,
    `${side}${PANEL_RAISED}${PANEL_TEXT} ${head} ${side}${canvas()}`,
    `${side}${PANEL_LINE}${"─".repeat(inner)}${side}${canvas()}`,
    ...body.map((row, index) => {
      const back = index === selected ? PANEL_SELECTED : PANEL_RAISED;
      const bar = index === selected ? PANEL_BLUE + "▌" + PANEL_TEXT : " ";
      return `${side}${back}${PANEL_TEXT}${bar}${fit(row, inner - 1, back)}${side}${canvas()}`;
    }),
    `${PANEL_RAISED}${PANEL_RULE}╰${"─".repeat(inner)}╯${canvas()}`,
  ];
}
