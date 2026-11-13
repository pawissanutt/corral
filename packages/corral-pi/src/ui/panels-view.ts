// Native presentation only. All values come from the bound host or an explicit Git read.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { ContextUsage, Theme } from "@earendil-works/pi-coding-agent";
import type { BuildPanelState } from "../build/types.ts";
import type { TodoProgress } from "./todo-widget.ts";
import type { AgentObservation } from "./agents-model.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import type { QueuedItem } from "../host/queue-control.ts";
import { ansi, tint, PACKAGED, type Appearance } from "./appearance.ts";

export interface PanelJob { readonly job: string; readonly state: string; readonly detail?: import("../extensions/jobs/runtime.ts").JobInspection }
export interface PanelSnapshot {
  readonly build?: BuildPanelState;
  readonly inspector?: InspectorSnapshot;
  readonly budget?: unknown;
  readonly agents?: readonly AgentObservation[];
  readonly bound: boolean;
  readonly model: string | null;
  readonly effort: string;
  readonly effortAvailable?: boolean;
  readonly input?: import("../host/input-observation.ts").InputObservation;
  readonly streaming: boolean;
  readonly context: ContextUsage | undefined;
  readonly tokens: number;
  /** API-equivalent USD of this session's responses, priced by the model catalog; absent means unknown. */
  readonly cost?: number;
  readonly tools: readonly string[];
  readonly contextFiles: readonly string[];
  readonly skills: readonly string[];
  readonly ledger: TodoProgress | null;
  readonly ledgerStale: boolean;
  readonly jobs: readonly PanelJob[] | null;
  readonly auxiliary: { explore: { running: number; queued: number }; oracle: { running: number; queued: number } } | null;
  readonly helpers?: readonly import("../extensions/subagent/inspection.ts").HelperInspection[] | null;
  readonly steering: number;
  readonly followUps: number;
  readonly queue?: readonly QueuedItem[];
}
export interface PanelFile { readonly path: string; readonly status: string; readonly originalPath?: string }
export interface PanelLine {
  readonly text: string;
  readonly heading?: boolean;
  readonly tone?: "text" | "muted" | "accent" | "success" | "warning";
  readonly indent?: number;
  readonly continuationIndent?: number;
}
export let PANEL_BG = ansi(PACKAGED.dark.background, true);
export let PANEL_TEXT = ansi(PACKAGED.dark.text);
export let PANEL_MUTED = ansi(PACKAGED.dark.muted);
export let PANEL_BLUE = ansi(PACKAGED.dark.accent);
export let PANEL_RULE = ansi(PACKAGED.dark.rule);
export let PANEL_SUCCESS = ansi(PACKAGED.dark.success);
export let PANEL_WARNING = ansi(PACKAGED.dark.warning);
export let PANEL_ERROR = ansi(PACKAGED.dark.error);
export let PANEL_SELECTED = ansi(PACKAGED.dark.selected, true);
/** Foreground of borders, rules and separators. */
export let PANEL_LINE = ansi(PACKAGED.dark.line);
/** Foreground of headings in details and popups. */
export let PANEL_HEAD = ansi(PACKAGED.dark.head);
/** Foreground of inline code. */
export let PANEL_CODE = ansi(PACKAGED.dark.code);
/** Background of a card's header row, popups and toasts. */
export let PANEL_RAISED = ansi(PACKAGED.dark.raised, true);
/** Background of a key cap. */
export let PANEL_KEY = ansi(PACKAGED.dark.key, true);
/** Background of a docked dialog's header row. */
export let PANEL_DIALOG = ansi(PACKAGED.dark.dialog, true);
/** Background of a user message. */
export let PANEL_BUBBLE = ansi(PACKAGED.dark.bubble, true);
/** The accent surface with dark bold text: the role pill and the active key cap. */
export let PANEL_ON_ACCENT = onAccent("dark");
function onAccent(appearance: Appearance): string {
  const p = PACKAGED[appearance];
  return ansi(p.accent, true) + ansi(p.onAccent) + "\x1b[1m";
}
/** Tinted backgrounds (chips, diff rows, hunk bars), derived from the same packaged palette. */
export interface PanelTints { readonly raised: string; readonly accent: string; readonly success: string; readonly warning: string; readonly error: string }
function panelTints(appearance: Appearance): PanelTints {
  const p = PACKAGED[appearance], bg = (color: string, amount: number) => ansi(tint(p.background, color, amount), true);
  return { raised: ansi(p.raised, true), accent: bg(p.accent, 0.12), success: bg(p.success, 0.14), warning: bg(p.warning, 0.14), error: bg(p.error, 0.14) };
}
export let PANEL_TINT = panelTints("dark");
/** One native host per process; live bindings keep every panel on the same packaged palette. */
export function setPanelAppearance(appearance: Appearance): void {
  const p = PACKAGED[appearance];
  PANEL_BG = ansi(p.background, true); PANEL_TEXT = ansi(p.text); PANEL_MUTED = ansi(p.muted);
  PANEL_BLUE = ansi(p.accent); PANEL_RULE = ansi(p.rule); PANEL_SUCCESS = ansi(p.success); PANEL_WARNING = ansi(p.warning); PANEL_ERROR = ansi(p.error); PANEL_SELECTED = ansi(p.selected, true);
  PANEL_LINE = ansi(p.line); PANEL_HEAD = ansi(p.head); PANEL_CODE = ansi(p.code); PANEL_RAISED = ansi(p.raised, true); PANEL_KEY = ansi(p.key, true);
  PANEL_DIALOG = ansi(p.dialog, true); PANEL_BUBBLE = ansi(p.bubble, true); PANEL_ON_ACCENT = onAccent(appearance);
  PANEL_TINT = panelTints(appearance);
}
export function styledPanelLine(row: PanelLine): string {
  const color = row.tone === "muted" ? PANEL_MUTED : row.tone === "success" ? PANEL_SUCCESS
    : row.tone === "warning" ? PANEL_WARNING : row.tone === "accent" ? PANEL_BLUE : PANEL_TEXT;
  return color + (row.heading ? "\x1b[1m" : "") + " ".repeat(row.indent ?? 0) + row.text + "\x1b[22m" + PANEL_TEXT;
}
export const RESET = "\x1b[0m";

export function plainLabel(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/gu, " ").trim();
}
export function pad(text: string, width: number): string {
  const clipped = truncateToWidth(text, Math.max(0, width), "…");
  return clipped + " ".repeat(Math.max(0, width - visibleWidth(clipped)));
}
/** A rounded box `width` by `height` with `title` set into its top border; the border is accent with the focus, else the line colour. */
export function box(title: string, lines: readonly string[], width: number, height: number, focus = false): string[] {
  if (width < 4 || height < 2) return [];
  const rule = focus ? PANEL_BLUE : PANEL_LINE;
  const heading = pad(` ${plainLabel(title)} `, width - 4).trimEnd();
  const result = [`${PANEL_BG}${rule}╭─${heading}${"─".repeat(Math.max(0, width - 3 - visibleWidth(heading)))}╮${RESET}`];
  for (let row = 0; row < height - 2; row++) {
    result.push(`${PANEL_BG}${rule}│${PANEL_TEXT} ${pad(lines[row] ?? "", width - 4)}${PANEL_BG} ${rule}│${RESET}`);
  }
  result.push(`${PANEL_BG}${rule}╰${"─".repeat(width - 2)}╯${RESET}`);
  return result;
}
export function diffLine(text: string, theme: Pick<Theme, "fg">, inHunk = false): string {
  const safe = text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
  if (!inHunk && /^(diff |index |---|\+\+\+)/.test(safe)) return theme.fg("muted", safe);
  if (safe.startsWith("@@")) return theme.fg("accent", safe);
  if (safe.startsWith("+")) return theme.fg("toolDiffAdded", safe);
  if (safe.startsWith("-")) return theme.fg("toolDiffRemoved", safe);
  return safe;
}
