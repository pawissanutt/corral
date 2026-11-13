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
/** Below this terminal width only the open tab keeps its label. */
const LABEL_COLUMNS = 50;
/** How long a notice replaces the state word in header row 1. */
export const NOTICE_MS = 3200;

export type ChromeTier = "wide" | "mid" | "narrow" | "tiny";
export function chromeTier(columns: number): ChromeTier {
  return columns >= WIDE_COLUMNS ? "wide" : columns >= RAIL_COLUMNS ? "mid" : columns >= COMPACT_COLUMNS ? "narrow" : "tiny";
}

export type TabId = "task" | "conversation" | "agents" | "changes" | "evidence" | "inbox" | "board";
export type BadgeTone = "text" | "accent" | "success" | "warning" | "error";
export interface TabSpec {
  readonly id: TabId; readonly key: string; readonly label: string; readonly short?: string;
  readonly badge?: { readonly text: string; readonly tone: BadgeTone };
}
export interface TabHit { readonly from: number; readonly to: number; readonly id: TabId }
export interface Notice { readonly text: string; readonly tone: "text" | "success" | "warning" | "error" }
/** A status that replaces the state word: `long` in roomy headers, `short` when tiny or tight. */
export interface HeaderStatus { readonly long: string; readonly short: string; readonly tone: BadgeTone | "muted"; readonly clickable: boolean }
export interface HeaderInput {
  readonly title: string; readonly state: "Working" | "Idle" | "Unavailable";
  readonly attention?: { readonly text: string; readonly tone: BadgeTone }; readonly notice?: Notice;
  readonly context?: { readonly left: string; readonly right?: string };
  /** Replaces the state word; a notice still wins for its duration. */
  readonly status?: HeaderStatus;
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
  inbox: { key: "F11", label: "Inbox" },
  board: { key: "F10", label: "Board" },
};
const TASK_TABS: readonly TabId[] = ["task", "conversation", "agents", "changes", "evidence", "inbox"];
const SESSION_TABS: readonly TabId[] = ["conversation", "agents", "changes", "inbox"];
const BUILD_TABS: readonly TabId[] = ["conversation", "agents", "changes", "board"];

/**
 * Task-bound roles keep Task and Proof; assistant and PM sessions have neither, and both have Inbox. A build
 * session has Board in Proof's place on F10 and no Inbox.
 */
export function mainTabs(identity: HostIdentity): readonly TabId[] {
  return identity.role === "build" ? BUILD_TABS : identity.task ? TASK_TABS : SESSION_TABS;
}
/**
 * The key cap and label of a main view tab, with an optional count badge. Chat takes none: whether the
 * session is working is the header's state word.
 */
export function tabSpec(id: TabId, badge?: TabSpec["badge"]): TabSpec {
  return { id, ...TAB_TEXT[id], ...(badge && id !== "conversation" ? { badge } : {}) };
}

/** No project chosen reads `?`, a failed Git read `!`; an unread or empty comparison has no badge, never `0`. */
export function changesBadge(input: { readonly checked: boolean; readonly files: number; readonly scopeRequired: boolean; readonly error: boolean }): TabSpec["badge"] {
  if (input.scopeRequired) return { text: "?", tone: "warning" };
  if (input.error) return { text: "!", tone: "warning" };
  return input.checked && input.files > 0 ? { text: String(input.files), tone: "text" } : undefined;
}

/** Board tasks not yet landed, in warning when one is blocked or failed; `◆` when a plan waits and nothing is filed. */
export function boardBadge(input: { readonly open: number; readonly alert: boolean; readonly proposed: boolean }): TabSpec["badge"] {
  if (input.open > 0) return { text: String(input.open), tone: input.alert ? "warning" : "accent" };
  return input.proposed ? { text: "◆", tone: "warning" } : undefined;
}

/**
 * Gate glyph then review glyph, so colour is never the only signal: gate `✓` fresh pass, `✗` fail,
 * `●` running, `…` other or stale, `–` none; review `R` fresh verdict, `…` pending or stale, `–` none.
 * A trailing `·` marks a stale read; `!` a failed read with nothing retained.
 */
export function proofBadge(input: {
  readonly gate?: { readonly result: string; readonly fresh: boolean };
  readonly review?: { readonly verdict?: string | null; readonly fresh: boolean };
  readonly stale: boolean; readonly error: boolean;
}): TabSpec["badge"] {
  const { gate, review } = input;
  if (!gate && !review) return input.error ? { text: "!", tone: "warning" } : undefined;
  const gateMark = !gate ? "–" : gate.result === "pass" && gate.fresh ? "✓" : gate.result === "fail" ? "✗" : gate.result === "running" ? "●" : "…";
  const reviewMark = !review ? "–" : !review.fresh || !review.verdict ? "…" : "R";
  const tone: BadgeTone = gateMark === "✗" ? "error"
    : gateMark === "…" || reviewMark === "…" || input.stale ? "warning"
    : gateMark === "●" ? "accent" : gateMark === "✓" && reviewMark === "R" ? "success" : "text";
  return { text: gateMark + reviewMark + (input.stale ? "·" : ""), tone };
}

interface Fit { readonly badges: boolean; readonly short: boolean; readonly only: boolean; readonly rules: boolean }
// Measured in order: count chips go, then `Changes` reads `Diff`, then separators, then only the open tab
// keeps its label. Below 64 columns the row starts without chips, separators and the long label; below 50
// only the open tab keeps its label.
const FITS: readonly Fit[] = [
  { badges: true, short: false, only: false, rules: true },
  { badges: false, short: false, only: false, rules: true },
  { badges: false, short: true, only: false, rules: true },
  { badges: false, short: true, only: false, rules: false },
  { badges: false, short: true, only: true, rules: false },
];

const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";
/** Underline in the accent colour (SGR 58), so the open tab's mark reads as accent under text-coloured letters. */
const accentUnderline = () => "\x1b[4m" + PANEL_BLUE.replace("\x1b[38;", "\x1b[58;");
const NO_UNDERLINE = "\x1b[59m\x1b[24m";

interface Piece { readonly id: TabId; readonly text: string; readonly width: number }
function tabPiece(tab: TabSpec, active: boolean, fit: Fit): Piece {
  const key = plainLabel(tab.key);
  const label = fit.only && !active ? "" : plainLabel(fit.short && tab.short ? tab.short : tab.label);
  const badge = fit.badges && tab.badge ? plainLabel(tab.badge.text) : "";
  let text = frameCap(key, active), width = visibleWidth(key) + 2;
  if (label) {
    text += " " + (active ? PANEL_TEXT + BOLD + accentUnderline() + label + NO_UNDERLINE + NORMAL : PANEL_MUTED + label);
    width += 1 + visibleWidth(label);
  }
  if (badge && tab.badge) {
    // A count reads in the text colour; warning, error and success keep their tone as the signal.
    const tone = tab.badge.tone === "text" || tab.badge.tone === "accent" ? "" : toneColor(tab.badge.tone);
    text += " " + countChip(tone + badge);
    width += 3 + visibleWidth(badge);
  }
  return { id: tab.id, text, width };
}

/**
 * One row of main view tabs, laid out for the full terminal width: key cap, label and count chip, separated
 * by `│` in the line colour. The open tab has the active key cap and a bold label underlined in accent;
 * the others a plain key cap and a muted label. Hits are columns of this row; clicking a tab opens it.
 */
export function tabRow(tabs: readonly TabSpec[], active: TabId | undefined, width: number):
  { readonly line: string; readonly hits: readonly TabHit[] } {
  const room = Math.max(0, width - 1);
  const first = width < LABEL_COLUMNS ? 4 : width < COMPACT_COLUMNS ? 3 : 0;
  const layout = (fit: Fit, shown: readonly TabSpec[]) => {
    const pieces = shown.map(tab => tabPiece(tab, tab.id === active, fit));
    const gap = fit.rules ? 3 : 1;
    return { pieces, gap, width: pieces.reduce((sum, piece) => sum + piece.width, 0) + gap * Math.max(0, pieces.length - 1) };
  };
  let fit = FITS[FITS.length - 1]!, chosen = layout(fit, tabs);
  for (const candidate of FITS.slice(first)) {
    const attempt = layout(candidate, tabs);
    if (attempt.width <= room) { fit = candidate; chosen = attempt; break; }
  }
  if (chosen.width > room) {
    // Still too wide: keep the open tab and as many neighbours as fit, nearest first.
    const centre = Math.max(0, tabs.findIndex(tab => tab.id === active));
    let from = centre, to = centre + 1;
    for (let grew = true; grew;) {
      grew = false;
      for (const [nextFrom, nextTo] of [[from, to + 1], [from - 1, to]] as const) {
        if (nextFrom < 0 || nextTo > tabs.length) continue;
        if (layout(fit, tabs.slice(nextFrom, nextTo)).width <= room) { from = nextFrom; to = nextTo; grew = true; break; }
      }
    }
    chosen = layout(fit, tabs.slice(from, to));
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

/**
 * Header row 1, laid out for the full terminal width: `corral / <target>` on the left (`<target>` alone
 * when tiny); at the right edge the attention chip, then the status or the state word, or a notice in its
 * place. Below 105 columns the status takes its short form and the state word is lower case; the long
 * status also gives way when it would squeeze the title. Row 2 (context, muted) shows only when neither
 * short nor tiny. Hits are columns of row 0: the chip, and the status when it is clickable and no notice
 * covers it.
 */
export function headerFrame(input: HeaderInput, width: number, options: { readonly short: boolean; readonly tiny: boolean }):
  { readonly lines: string[]; readonly hits: readonly HeaderHit[] } {
  const inner = Math.max(0, width - 2);
  const label = (options.tiny ? "" : "corral / ") + plainLabel(input.title);
  const title = "\x1b[1m" + PANEL_BLUE + label + "\x1b[22m";
  const chipText = input.attention ? " " + plainLabel(input.attention.text) + " " : "";
  const chipWidth = visibleWidth(chipText), chipGap = chipText ? 1 : 0;
  const keep = Math.min(visibleWidth(label), TITLE_KEEP);
  const status = input.notice || !input.status ? undefined : input.status;
  const small = options.tiny || width < RAIL_COLUMNS;
  const statusText = input.notice ? plainLabel(input.notice.text) : !status ? (small ? input.state.toLowerCase() : input.state)
    : !small && chipWidth + chipGap + visibleWidth(plainLabel(status.long)) + 2 + keep <= inner ? plainLabel(status.long) : plainLabel(status.short);
  const state = (input.notice ? toneColor(input.notice.tone) : status ? toneColor(status.tone) : PANEL_MUTED) + statusText;
  const chip = input.attention
    ? toneColor(input.attention.tone) + "\x1b[7m\x1b[1m" + chipText + "\x1b[22m\x1b[27m" + PANEL_BG + " "
    : "";
  const lines = [chromeLine(" " + splitRow(title, chip + state, inner), width)];
  if (!options.short && !options.tiny && input.context) {
    lines.push(chromeLine(" " + PANEL_MUTED + splitRow(plainLabel(input.context.left), plainLabel(input.context.right ?? ""), inner), width));
  }
  // splitRow right-aligns the chip and status within `inner`, after the row's one-cell left inset.
  const right = chipWidth + chipGap + visibleWidth(statusText), end = 1 + inner, start = end - Math.min(inner, right);
  const hits: HeaderHit[] = [];
  const add = (id: HeaderHit["id"], from: number, size: number) => {
    const to = Math.min(end, from + size);
    if (from < to) hits.push({ row: 0, from, to, id });
  };
  if (input.attention) add("attention", start, chipWidth);
  if (status?.clickable) add("status", start + chipWidth + chipGap, visibleWidth(statusText));
  return { lines, hits };
}

/** The header rows alone; see `headerFrame`. */
export function headerRows(input: HeaderInput, width: number, options: { readonly short: boolean; readonly tiny: boolean }): string[] {
  return headerFrame(input, width, options).lines;
}
