// Main-column chrome: the two header rows and the main view tab row, plus the width and height tiers
// every panel reads. Pure rendering; the workbench owns state, notice timing and pointer routing.
import { visibleWidth } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { countChip, keyCap as frameCap } from "./frame.ts";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_KEY, PANEL_LINE, PANEL_MUTED, PANEL_ON_ACCENT, PANEL_SUCCESS,
  PANEL_TEXT, PANEL_WARNING, RESET,
} from "./panels-view.ts";

/** Physical columns where the sidebar becomes a rail, the wide tier starts, and compact chrome ends. */
export const RAIL_COLUMNS = 105, WIDE_COLUMNS = 150, COMPACT_COLUMNS = 64, SHORT_ROWS = 28, VERY_SHORT_ROWS = 20;
/** How long a notice replaces the state word in header row 1. */
export const NOTICE_MS = 3200;

export type ChromeTier = "wide" | "mid" | "narrow" | "tiny";
export function chromeTier(columns: number): ChromeTier {
  return columns >= WIDE_COLUMNS ? "wide" : columns >= RAIL_COLUMNS ? "mid" : columns >= COMPACT_COLUMNS ? "narrow" : "tiny";
}

export type TabId = "task" | "conversation" | "agents" | "changes" | "evidence" | "board";
export type BadgeTone = "text" | "accent" | "success" | "warning" | "error";
/** A tab's badge: `text` where the row has room, `short` (when given) once it must shorten. */
export interface TabBadge { readonly text: string; readonly short?: string; readonly tone: BadgeTone }
export interface TabSpec {
  readonly id: TabId; readonly key: string; readonly label: string; readonly short?: string;
  readonly badge?: TabBadge;
  /** New content the owner has not seen: an accent `•` after the label that survives every fit tier. */
  readonly unread?: boolean;
}
export interface TabHit { readonly from: number; readonly to: number; readonly id: TabId }
export interface Notice { readonly text: string; readonly tone: "text" | "success" | "warning" | "error" }
/** A status that replaces the state word: `long` in roomy headers, `short` when tiny or tight. */
export interface HeaderStatus { readonly long: string; readonly short: string; readonly tone: BadgeTone | "muted"; readonly clickable: boolean }
export interface HeaderInput {
  readonly title: string; readonly state: "Working" | "Idle" | "Unavailable";
  /** The reversed chip at the right edge: `text` when it fits (`▲ 2 needs you`), else `short` (`▲ 2`). */
  readonly attention?: { readonly text: string; readonly short?: string; readonly tone: BadgeTone }; readonly notice?: Notice;
  /** Row 2: `left` keeps most of the row; `right` reads `rightShort` (when given) once both do not fit. */
  readonly context?: { readonly left: string; readonly right?: string; readonly rightShort?: string };
  /** Replaces the state word; a notice still wins for its duration. */
  readonly status?: HeaderStatus;
  /** While the agent works: a styled one-cell glyph before the state word or status (the spinner), and the turn's elapsed time. */
  readonly mark?: { readonly glyph: string; readonly elapsed?: string };
}
/** Columns of header row `row` that a click acts on: the attention chip, or a clickable status. */
export interface HeaderHit { readonly row: number; readonly from: number; readonly to: number; readonly id: "attention" | "status" }

function toneColor(tone: BadgeTone | Notice["tone"] | "muted"): string {
  return tone === "accent" ? PANEL_BLUE : tone === "success" ? PANEL_SUCCESS : tone === "warning" ? PANEL_WARNING
    : tone === "error" ? PANEL_ERROR : tone === "muted" ? PANEL_MUTED : PANEL_TEXT;
}

/**
 * One chrome row of exactly `width` cells on the panel background. Truncation ends with a full reset,
 * so the background is restored after it and clipped rows never show black cells.
 */
export function chromeLine(text: string, width: number): string {
  return PANEL_BG + PANEL_TEXT + pad(text, Math.max(0, width)).replaceAll(RESET, RESET + PANEL_BG) + RESET;
}

/**
 * A key cap with the options the footer and lists use: `frame.ts` draws it (accent on the key surface, or
 * the active form on the accent surface); `tight` drops the one-cell padding. `restore` is the background
 * the row continues with.
 */
export function keyCap(key: string, options: { readonly active?: boolean; readonly tight?: boolean; readonly restore?: string } = {}): string {
  if (!options.tight) return frameCap(key, options.active ?? false, options.restore);
  const restore = options.restore ?? PANEL_BG;
  return options.active ? PANEL_ON_ACCENT + key + "\x1b[22m" + restore + PANEL_TEXT : PANEL_KEY + PANEL_BLUE + key + restore + PANEL_TEXT;
}
export function keyCapWidth(key: string, tight = false): number {
  return visibleWidth(key) + (tight ? 0 : 2);
}

const TAB_TEXT: Readonly<Record<TabId, { readonly key: string; readonly label: string; readonly short?: string }>> = {
  task: { key: "F5", label: "Task" },
  conversation: { key: "F9", label: "Chat" },
  agents: { key: "F8", label: "Agents" },
  changes: { key: "F7", label: "Changes", short: "Diff" },
  evidence: { key: "F10", label: "Proof" },
  board: { key: "F10", label: "Board" },
};
const TASK_TABS: readonly TabId[] = ["task", "conversation", "agents", "changes", "evidence"];
const SESSION_TABS: readonly TabId[] = ["conversation", "agents", "changes"];
const BUILD_TABS: readonly TabId[] = ["conversation", "agents", "changes", "board"];

/** Task-bound roles keep Task and Proof; assistant sessions have neither. A build session has Board in Proof's place on F10. */
export function mainTabs(identity: HostIdentity): readonly TabId[] {
  return identity.role === "build" ? BUILD_TABS : identity.task ? TASK_TABS : SESSION_TABS;
}
/**
 * The key cap and label of a main view tab, with an optional count badge and the unread dot. Chat takes no
 * badge (whether the session is working is the header's state word); only Chat is ever unread.
 */
export function tabSpec(id: TabId, badge?: TabBadge, unread = false): TabSpec {
  return { id, ...TAB_TEXT[id], ...(badge && id !== "conversation" ? { badge } : {}), ...(unread ? { unread } : {}) };
}

/** No project chosen reads `project?`, a failed Git read `git?`; an unread or empty comparison has no badge, never `0`. */
export function changesBadge(input: { readonly checked: boolean; readonly files: number; readonly scopeRequired: boolean; readonly error: boolean }): TabBadge | undefined {
  if (input.scopeRequired) return { text: "project?", tone: "warning" };
  if (input.error) return { text: "git?", tone: "warning" };
  return input.checked && input.files > 0 ? { text: String(input.files), tone: "text" } : undefined;
}

/** Board tasks not yet landed, in warning when one is blocked or failed; `◆` when a plan waits and nothing is filed. */
export function boardBadge(input: { readonly open: number; readonly alert: boolean; readonly proposed: boolean }): TabBadge | undefined {
  if (input.open > 0) return { text: String(input.open), tone: input.alert ? "warning" : "accent" };
  return input.proposed ? { text: "◆", tone: "warning" } : undefined;
}

/**
 * The newest gate in words, so colour is never the only signal: `gate ✓` a pass, `gate ✗` a failure, `no gate`.
 * The long form adds the newest review where the row has room (`approved` or `changes`) and ` · stale` for a
 * stale read; the short form keeps the gate and `stale`. A failed read with nothing retained reads `proof?`.
 */
export function proofBadge(input: {
  readonly gate?: { readonly result: "pass" | "fail" };
  readonly review?: { readonly result: "pass" | "fail" };
  readonly stale: boolean; readonly error: boolean;
}): TabBadge | undefined {
  const { gate, review } = input;
  if (!gate && !review) return input.error ? { text: "proof?", tone: "warning" } : undefined;
  const mark = !gate ? "" : gate.result === "pass" ? "✓" : "✗";
  const gateText = gate ? `gate ${mark}` : "no gate";
  const reviewText = !review ? "" : review.result === "pass" ? "approved" : "changes";
  const tone: BadgeTone = mark === "✗" ? "error"
    : reviewText === "changes" || input.stale ? "warning"
    : mark === "✓" && reviewText === "approved" ? "success" : "text";
  const text = [gateText, reviewText].filter(Boolean).join(" · ") + (input.stale ? " · stale" : "");
  const short = gateText + (input.stale ? " stale" : "");
  return text === short ? { text, tone } : { text, short, tone };
}

/**
 * One fit tier of the tab row. Measured in order: long badges shorten, then the separators go, then
 * `Changes` reads `Diff`, then inactive tabs lose their labels (a badge stays on the bare key cap, `F11 3`).
 * Badges go last, one at a time; the unread dot never goes.
 */
interface Fit { readonly long: boolean; readonly rules: boolean; readonly short: boolean; readonly only: boolean }
const LAST_FIT: Fit = { long: false, rules: false, short: true, only: true };
const FITS: readonly Fit[] = [
  { long: true, rules: true, short: false, only: false },
  { long: false, rules: true, short: false, only: false },
  { long: false, rules: false, short: false, only: false },
  { long: false, rules: false, short: true, only: false },
  LAST_FIT,
];

const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";
/** Underline in the accent colour (SGR 58), so the open tab's mark reads as accent under text-coloured letters. */
const accentUnderline = () => "\x1b[4m" + PANEL_BLUE.replace("\x1b[38;", "\x1b[58;");
const NO_UNDERLINE = "\x1b[59m\x1b[24m";

interface Piece { readonly id: TabId; readonly text: string; readonly width: number }
function tabPiece(tab: TabSpec, active: boolean, fit: Fit, badged: boolean, focused: boolean): Piece {
  const key = plainLabel(tab.key);
  const label = fit.only && !active ? "" : plainLabel(fit.short && tab.short ? tab.short : tab.label);
  const badge = badged && tab.badge ? plainLabel(fit.long ? tab.badge.text : tab.badge.short ?? tab.badge.text) : "";
  // The open tab's cap is the accent cap only while its view has the keys; else a muted cap on the key surface.
  let text = active && !focused ? `${PANEL_KEY}${PANEL_MUTED} ${key} ${PANEL_BG}${PANEL_TEXT}` : frameCap(key, active);
  let width = visibleWidth(key) + 2;
  if (label) {
    text += " " + (active ? PANEL_TEXT + BOLD + accentUnderline() + label + NO_UNDERLINE + NORMAL : PANEL_MUTED + label);
    width += 1 + visibleWidth(label);
  }
  if (tab.unread) { text += " " + PANEL_BLUE + "•"; width += 2; }
  if (badge && tab.badge) {
    // A count reads in the text colour; warning, error and success keep their tone as the signal.
    const tone = tab.badge.tone === "text" || tab.badge.tone === "accent" ? "" : toneColor(tab.badge.tone);
    text += " " + countChip(tone + badge);
    width += 3 + visibleWidth(badge);
  }
  return { id: tab.id, text, width };
}

/** Badges in the order the narrowest tier keeps them: warnings and errors first, then the rest in tab order. */
function badgeOrder(tabs: readonly TabSpec[]): TabId[] {
  const alarm = (tab: TabSpec) => tab.badge?.tone === "warning" || tab.badge?.tone === "error";
  return [...tabs.filter(tab => tab.badge && alarm(tab)), ...tabs.filter(tab => tab.badge && !alarm(tab))].map(tab => tab.id);
}

/**
 * One row of main view tabs, laid out for the full terminal width: key cap, label, unread dot and badge,
 * separated by `│` in the line colour. The open tab has the active key cap and a bold label underlined in
 * accent; the others a plain key cap and a muted label. While the open view does not have the keys (`focused`
 * false) its cap is muted on the key surface and its label stays. Hits are columns of this row; clicking a tab
 * opens it.
 */
export function tabRow(tabs: readonly TabSpec[], active: TabId | undefined, width: number, focused = true):
  { readonly line: string; readonly hits: readonly TabHit[] } {
  const room = Math.max(0, width - 1);
  const layout = (fit: Fit, shown: readonly TabSpec[], badged: ReadonlySet<TabId>) => {
    const pieces = shown.map(tab => tabPiece(tab, tab.id === active, fit, badged.has(tab.id), focused));
    const gap = fit.rules ? 3 : 1;
    return { pieces, gap, width: pieces.reduce((sum, piece) => sum + piece.width, 0) + gap * Math.max(0, pieces.length - 1) };
  };
  const all = new Set(tabs.filter(tab => tab.badge).map(tab => tab.id));
  let chosen: ReturnType<typeof layout> | undefined;
  for (const fit of FITS) {
    const attempt = layout(fit, tabs, all);
    if (attempt.width <= room) { chosen = attempt; break; }
  }
  if (!chosen) {
    // Badges go last, one at a time: keep as many as fit, warnings and errors first.
    const kept = new Set<TabId>();
    for (const id of badgeOrder(tabs)) {
      kept.add(id);
      if (layout(LAST_FIT, tabs, kept).width > room) kept.delete(id);
    }
    chosen = layout(LAST_FIT, tabs, kept);
  }
  if (chosen.width > room) {
    // Still too wide: keep the open tab and as many neighbours as fit, nearest first.
    const none = new Set<TabId>();
    const centre = Math.max(0, tabs.findIndex(tab => tab.id === active));
    let from = centre, to = centre + 1;
    for (let grew = true; grew;) {
      grew = false;
      for (const [nextFrom, nextTo] of [[from, to + 1], [from - 1, to]] as const) {
        if (nextFrom < 0 || nextTo > tabs.length) continue;
        if (layout(LAST_FIT, tabs.slice(nextFrom, nextTo), none).width <= room) { from = nextFrom; to = nextTo; grew = true; break; }
      }
    }
    chosen = layout(LAST_FIT, tabs.slice(from, to), none);
  }
  const separator = chosen.gap === 3 ? " " + PANEL_LINE + "│ " : " ";
  const hits: TabHit[] = [];
  let x = 1;
  for (const piece of chosen.pieces) {
    if (x < width) hits.push({ from: x, to: Math.min(width, x + piece.width), id: piece.id });
    x += piece.width + chosen.gap;
  }
  return { line: chromeLine(" " + chosen.pieces.map(piece => piece.text).join(separator), width), hits };
}

/** The title keeps at least this many cells before a long status gives way to its short form. */
const TITLE_KEEP = 24;
/** Row 2: the left part keeps at least this share of the row before the right part is clipped or dropped. */
const CONTEXT_KEEP = 0.6;
/** Row 2's right part shows only with at least this many cells. */
const CONTEXT_MIN = 6;

/**
 * Row 2's text within `inner` cells: `left` and `right` apart when both fit (`rightShort` once `right` does
 * not); else the left keeps 60% of the row (or all it needs) and the short right part is clipped into the rest,
 * or dropped when fewer than 6 cells remain.
 */
function contextText(context: NonNullable<HeaderInput["context"]>, inner: number): string {
  const left = plainLabel(context.left), leftWidth = visibleWidth(left);
  const forms = [context.right, context.rightShort].map(text => plainLabel(text ?? "")).filter(Boolean);
  for (const right of forms) if (leftWidth + 2 + visibleWidth(right) <= inner) return splitRow(left, right, inner);
  const right = forms[forms.length - 1] ?? "";
  const space = inner - Math.min(leftWidth, Math.ceil(inner * CONTEXT_KEEP)) - 2;
  if (!right || space < CONTEXT_MIN) return pad(left, inner);
  const rightWidth = Math.min(visibleWidth(right), space);
  return pad(left, inner - rightWidth - 2) + "  " + pad(right, rightWidth);
}

/**
 * Header row 1, laid out for the full terminal width: `corral / <target>` on the left (`<target>` alone
 * when tiny); at the right edge the attention chip (its long form when it fits), then the status or the
 * state word, or a notice in its place when the notice fits whole beside the short chip and the kept title
 * (a longer one is left to the toast). Below 105 columns the status takes its short form and the state
 * word is lower case; the long status also gives way when it would squeeze the title. Row 2 (context,
 * muted, clipped) shows whenever the terminal is not short, at every width. Hits are columns of row 0: the
 * chip, and the status when it is clickable and no notice covers it. `noticeShown` says whether row 1 drew
 * `input.notice`.
 */
export function headerFrame(input: HeaderInput, width: number, options: { readonly short: boolean; readonly tiny: boolean }):
  { readonly lines: string[]; readonly hits: readonly HeaderHit[]; readonly noticeShown: boolean } {
  const inner = Math.max(0, width - 2);
  const label = (options.tiny ? "" : "corral / ") + plainLabel(input.title);
  const title = "\x1b[1m" + PANEL_BLUE + label + "\x1b[22m";
  const chipOf = (text: string | undefined) => text ? " " + plainLabel(text) + " " : "";
  const chipShort = chipOf(input.attention?.short ?? input.attention?.text), chipLong = chipOf(input.attention?.text);
  const chipGap = chipShort ? 1 : 0;
  const keep = Math.min(visibleWidth(label), TITLE_KEEP);
  const notice = input.notice && visibleWidth(chipShort) + chipGap + visibleWidth(plainLabel(input.notice.text)) + 2 + keep <= inner
    ? input.notice : undefined;
  const status = notice || !input.status ? undefined : input.status;
  const mark = notice ? undefined : input.mark, markWidth = mark ? visibleWidth(mark.glyph) + 1 : 0;
  const small = options.tiny || width < RAIL_COLUMNS;
  const word = (small ? input.state.toLowerCase() : input.state) + (mark?.elapsed && !options.tiny ? " · " + mark.elapsed : "");
  const statusText = notice ? plainLabel(notice.text) : !status ? word
    : !small && visibleWidth(chipShort) + chipGap + markWidth + visibleWidth(plainLabel(status.long)) + 2 + keep <= inner ? plainLabel(status.long) : plainLabel(status.short);
  // The chip says what it counts (`▲ 2 needs you`) while that leaves the title its room, else its short form.
  const chipText = visibleWidth(chipLong) + chipGap + markWidth + visibleWidth(statusText) + 2 + keep <= inner ? chipLong : chipShort;
  const chipWidth = visibleWidth(chipText);
  const state = (mark ? mark.glyph + " " : "") + (notice ? toneColor(notice.tone) : status ? toneColor(status.tone) : PANEL_MUTED) + statusText;
  const chip = input.attention
    ? toneColor(input.attention.tone) + "\x1b[7m\x1b[1m" + chipText + "\x1b[22m\x1b[27m" + PANEL_BG + " "
    : "";
  const lines = [chromeLine(" " + splitRow(title, chip + state, inner), width)];
  if (!options.short && input.context) lines.push(chromeLine(" " + PANEL_MUTED + contextText(input.context, inner), width));
  // splitRow right-aligns the chip and status within `inner`, after the row's one-cell left inset.
  const right = chipWidth + chipGap + markWidth + visibleWidth(statusText), end = 1 + inner, start = end - Math.min(inner, right);
  const hits: HeaderHit[] = [];
  const add = (id: HeaderHit["id"], from: number, size: number) => {
    const to = Math.min(end, from + size);
    if (from < to) hits.push({ row: 0, from, to, id });
  };
  if (input.attention) add("attention", start, chipWidth);
  if (status?.clickable) add("status", start + chipWidth + chipGap + markWidth, visibleWidth(statusText));
  return { lines, hits, noticeShown: notice !== undefined };
}

/** The header rows alone; see `headerFrame`. */
export function headerRows(input: HeaderInput, width: number, options: { readonly short: boolean; readonly tiny: boolean }): string[] {
  return headerFrame(input, width, options).lines;
}
