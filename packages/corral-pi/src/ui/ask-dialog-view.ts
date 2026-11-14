// The answer card for ask_user and owner questions: the docked accent card of the other owner dialogs, with one
// tab per question plus Review in its header, numbered answer rows with the accent selection bar, and one key
// row of clickable caps below. Pure drawing: AskDialog (host/ask-user-dialog.ts) keeps every answer rule and
// hands this a read-only view of its state.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { card } from "./frame.ts";
import { dialogHintRow } from "./owner-dialog-parts.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";

export interface AskDialogTab { readonly label: string; readonly answered: boolean; readonly active: boolean; readonly submit: boolean }
export interface AskDialogOption { readonly label: string; readonly description?: string; readonly on: boolean; readonly cursor: boolean }
/** The Other / Note row. */
export interface AskDialogText {
  readonly kind: "other" | "note"; readonly cursor: boolean; readonly on: boolean; readonly draft: string;
  /** The live input row while the cursor is on it. */
  renderInput(width: number): string;
}
/** A key the dialog acts on now; `data` is what a click on its cap replays through the dialog's own input. */
export interface AskDialogKey { readonly key: string; readonly label: string; readonly data: string }
export interface AskDialogView {
  /** One per question, then the review tab. */
  readonly tabs: readonly AskDialogTab[];
  readonly question?: {
    readonly prompt: string; readonly multiSelect: boolean;
    readonly options: readonly AskDialogOption[];
    /** Absent when freeText is false. */
    readonly own?: AskDialogText;
  };
  /** The review tab: every question with its answer in full (undefined = unanswered). `required`: Enter cannot send while any is unanswered. */
  readonly review?: {
    readonly rows: readonly { readonly label: string; readonly answer: string | undefined }[];
    readonly unanswered: number; readonly required: boolean;
  };
  /** Keys that act now, most important first. */
  readonly keys: readonly AskDialogKey[];
}

export interface AskDialogFrame {
  readonly lines: string[];
  /** The key row (the last line) and its caps, for mouse replay. */
  readonly hint: { readonly row: number; readonly hits: readonly { readonly from: number; readonly to: number; readonly data: string }[] };
  /** The body window: its first row, the last first row it can have (0 when nothing is clipped), the rows a page moves. */
  readonly scroll: { readonly start: number; readonly max: number; readonly page: number };
}

/** From this width an option takes one row when its label and description fit the columns. */
const ONE_ROW = 84;
const LABEL_MAX = 26;
/** Below this the tab strip is the open tab alone; from it the answered count joins the header. */
const COUNT_FROM = 70;
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m", UNDER = "\x1b[4m", NOUNDER = "\x1b[24m";
/** Card borders, header, key row. */
const CHROME = 4;
/** Below this terminal width the card drops its one-cell margins (as the owner card does). */
const MARGIN_FROM = 12;
const control = (text: string) => text.replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ");

function tabCell(tab: AskDialogTab): string {
  const glyph = tab.submit ? "" : tab.answered ? PANEL_SUCCESS + "✓ " : PANEL_MUTED + "○ ";
  const label = plainLabel(tab.label);
  const text = tab.active ? BOLD + UNDER + PANEL_BLUE + label + NOUNDER + NORMAL
    : (tab.answered ? PANEL_SUCCESS : PANEL_MUTED) + label;
  return glyph + text + PANEL_TEXT;
}

/** The header: the tab strip, or below its room the open tab alone as `Q2/3 · Rollout`; the answered count at the right. */
function header(tabs: readonly AskDialogTab[], width: number): { readonly left: string; readonly right?: string } {
  const inner = width - 4;
  const strip = tabs.map(tabCell).join("   ");
  const at = Math.max(0, tabs.findIndex(tab => tab.active));
  const active = tabs[at]!;
  const questions = tabs.length - 1;
  const compact = (active.submit ? tabCell(active)
    : (active.answered ? PANEL_SUCCESS + "✓ " : PANEL_MUTED + "○ ") + BOLD + PANEL_BLUE + `Q${at + 1}/${questions} · ${plainLabel(active.label)}` + NORMAL + PANEL_TEXT)
    + (at < tabs.length - 1 ? PANEL_MUTED + " ›" + PANEL_TEXT : "");
  const lead = at > 0 ? PANEL_MUTED + "‹ " + PANEL_TEXT : "";
  const count = tabs.filter(tab => tab.answered).length;
  const note = `${count}/${questions} answered`;
  const withNote = width >= COUNT_FROM;
  const fits = (left: string, extra: number) => visibleWidth(left) + extra <= inner;
  if (withNote && fits(strip, note.length + 2)) return { left: strip, right: PANEL_MUTED + note };
  if (fits(strip, 0)) return { left: strip };
  if (withNote && fits(lead + compact, note.length + 2)) return { left: lead + compact, right: PANEL_MUTED + note };
  return { left: lead + compact };
}

/**
 * The rows `[start, end)` of `total` that fit `room`, keeping rows `[from, to]` and the row after them in view; a
 * `manual` first row replaces that. `max` is the last first row the window can have and `page` the rows a page moves.
 */
function windowRows(total: number, from: number, to: number, room: number, manual?: number):
  { readonly start: number; readonly end: number; readonly above: number; readonly below: number; readonly max: number; readonly page: number } {
  if (total <= room) return { start: 0, end: total, above: 0, below: 0, max: 0, page: 1 };
  // Both edge markers are reserved first; a marker not needed gives its row back to content.
  const size = Math.max(1, room - 2), wanted = Math.min(total - 1, to + 1), max = total - size;
  let start = manual === undefined ? Math.max(0, Math.min(from, wanted - size + 1, max)) : Math.max(0, Math.min(manual, max)), end = start + size;
  if (start === 0) end = Math.min(total, end + 1);
  else if (end === total) start = Math.max(0, start - 1);
  return { start, end, above: start, below: total - end, max, page: Math.max(1, size - 1) };
}

interface Body { readonly rows: string[]; readonly from: number; readonly to: number }

function questionBody(view: NonNullable<AskDialogView["question"]>, inner: number, width: number): Body {
  const rows: string[] = [];
  const suffix = view.multiSelect ? PANEL_MUTED + " · choose any" : "";
  const asked = view.prompt.split(/\r\n|\r|\n/).map(control).join("\n") + NORMAL + suffix;
  for (const line of wrapTextWithAnsi(BOLD + PANEL_TEXT + asked, Math.max(1, inner))) rows.push(line);
  rows.push("");
  const total = view.options.length + (view.own ? 1 : 0);
  const numW = String(total).length + 1, col = numW + 2;
  const labels = view.options.map(option => plainLabel(option.label));
  const details = view.options.map(option => option.description ? plainLabel(option.description) : "");
  const widest = Math.max(0, ...labels.map(visibleWidth));
  const one = width >= ONE_ROW && widest <= LABEL_MAX
    && details.every(detail => visibleWidth(detail) <= inner - col - widest - 2);
  let from = -1, to = -1;
  const focus = (start: number) => { from = start; to = rows.length - 1; };
  const number = (index: number) => PANEL_BLUE + String(index + 1).padEnd(numW);
  view.options.forEach((option, index) => {
    const start = rows.length;
    const mark = (option.on ? PANEL_BLUE + "● " : PANEL_MUTED + "○ ");
    const head = number(index) + mark + PANEL_TEXT + BOLD;
    if (one) {
      rows.push(head + labels[index]!.padEnd(widest) + NORMAL + (details[index] ? "  " + PANEL_MUTED + details[index] : ""));
    } else {
      const label = wrapTextWithAnsi(labels[index]!, Math.max(1, inner - col));
      label.forEach((line, at) => rows.push((at === 0 ? head : " ".repeat(col) + PANEL_TEXT + BOLD) + line + NORMAL));
      if (details[index]) for (const line of wrapTextWithAnsi(details[index]!, Math.max(1, inner - col))) rows.push(" ".repeat(col) + PANEL_MUTED + line);
    }
    if (option.cursor) focus(start);
  });
  const text = view.own;
  if (text) {
    const start = rows.length;
    const mark = text.kind === "note" ? "  " : (text.on ? PANEL_BLUE + "● " : PANEL_MUTED + "○ ");
    const name = text.kind === "note" ? "Note" : "Other";
    const lead = number(view.options.length) + mark + PANEL_TEXT + BOLD + name + NORMAL;
    if (text.cursor) {
      const head = lead + "  " + PANEL_BLUE + "› " + PANEL_TEXT;
      const room = inner - visibleWidth(head);
      if (room >= 12) rows.push(head + text.renderInput(room));
      else {
        rows.push(lead);
        rows.push(" ".repeat(col) + PANEL_BLUE + "› " + PANEL_TEXT + text.renderInput(Math.max(1, inner - col - 2)));
      }
      focus(start);
    } else if (text.draft) {
      const head = lead + "  ";
      rows.push(head + PANEL_TEXT + truncateToWidth(text.draft, Math.max(1, inner - visibleWidth(head)), "…"));
    } else {
      rows.push(lead.replace(name + NORMAL, name + "…" + NORMAL) + "  " + PANEL_MUTED + (text.kind === "note" ? "add a note" : "type your own answer"));
    }
  }
  return { rows, from, to };
}

function reviewBody(view: NonNullable<AskDialogView["review"]>, inner: number): Body {
  const rows: string[] = [];
  const labels = view.rows.map(row => plainLabel(row.label));
  const widest = Math.max(0, ...labels.map(visibleWidth));
  const side = widest <= Math.min(16, Math.floor(inner / 3));
  const col = side ? widest + 2 : 2;
  view.rows.forEach((row, index) => {
    const label = PANEL_MUTED + labels[index]!;
    const answer = row.answer === undefined
      ? [PANEL_WARNING + "○ unanswered"]
      : wrapTextWithAnsi(BOLD + PANEL_TEXT + control(row.answer), Math.max(1, inner - col));
    if (side) answer.forEach((line, at) => rows.push((at === 0 ? label + " ".repeat(col - visibleWidth(labels[index]!)) : " ".repeat(col)) + line + NORMAL));
    else {
      rows.push(label);
      answer.forEach(line => rows.push(" ".repeat(col) + line + NORMAL));
    }
  });
  rows.push("");
  const action = view.required && view.unanswered > 0
    ? PANEL_TEXT + BOLD + "Go to the first unanswered" + NORMAL + PANEL_MUTED + ` · ${view.unanswered} left`
    : PANEL_TEXT + BOLD + "Send answers" + NORMAL + (view.unanswered > 0 ? PANEL_MUTED + ` · ${view.unanswered} unanswered, not sent` : "");
  rows.push(action);
  return { rows, from: rows.length - 1, to: rows.length - 1 };
}

/**
 * The card and its key row in at most `rows` lines of `width` cells. A body taller than the rows left shows a window of
 * it with a muted `↑ n more` / `↓ n more` row at each clipped edge. The window follows the cursor unless `scroll` (a
 * first body row the owner paged to) is given; `PgUp`/`PgDn` caps appear while rows are clipped. The returned `scroll`
 * is the window's first row, the last first row it can have, and the rows one page moves.
 */
export function askDialogFrame(view: AskDialogView, width: number, rows = Number.POSITIVE_INFINITY, scroll?: number): AskDialogFrame {
  // Like the owner card: one blank cell each side from this width, so the card sits in from the terminal edges and the
  // key row (full width, its caps from the second cell) lines up under the card's left border.
  const outer = width >= MARGIN_FROM ? 1 : 0, cardWidth = width - outer * 2;
  if (cardWidth < 8) return { lines: [], hint: { row: 0, hits: [] }, scroll: { start: 0, max: 0, page: 1 } };
  const inner = cardWidth - 4;
  const body = view.question ? questionBody(view.question, inner, cardWidth)
    : view.review ? reviewBody(view.review, inner) : { rows: [], from: -1, to: -1 };
  const room = Number.isFinite(rows) ? Math.max(3, Math.floor(rows) - CHROME) : body.rows.length;
  const shown = body.from < 0 ? windowRows(body.rows.length, 0, 0, room, scroll) : windowRows(body.rows.length, body.from, body.to, room, scroll);
  const visible = [
    ...(shown.above ? [PANEL_MUTED + `↑ ${shown.above} more`] : []),
    ...body.rows.slice(shown.start, shown.end),
    ...(shown.below ? [PANEL_MUTED + `↓ ${shown.below} more`] : []),
  ];
  const lead = shown.above ? 1 : 0;
  const head = header(view.tabs, cardWidth);
  const framed = card(head.left, visible, cardWidth, {
    tone: "accent", ...(head.right ? { right: head.right } : {}),
    ...(body.from >= 0 && body.from >= shown.start && body.from < shown.end
      ? { selected: { from: body.from - shown.start + lead, to: Math.min(body.to, shown.end - 1) - shown.start + lead } } : {}),
  });
  const paging: AskDialogKey[] = [
    ...(shown.below ? [{ key: "PgDn", label: "more", data: "\x1b[6~" }] : []),
    ...(shown.above ? [{ key: "PgUp", label: "back", data: "\x1b[5~" }] : []),
  ];
  const hints = dialogHintRow([...view.keys, ...paging], width);
  const framedLines = outer ? framed.map((line) => PANEL_BG + " " + line + PANEL_BG + " " + PANEL_TEXT) : framed;
  const out = [...framedLines, truncateToWidth(hints.line, width)];
  return { lines: out, hint: { row: out.length - 1, hits: hints.hits }, scroll: { start: shown.start, max: shown.max, page: shown.page } };
}

export function askDialogLines(view: AskDialogView, width: number, rows?: number): string[] {
  return askDialogFrame(view, width, rows).lines;
}
