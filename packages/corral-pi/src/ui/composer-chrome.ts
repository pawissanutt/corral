// The composer's own chrome: a rounded box one cell in from each terminal edge, its title set into the top
// border, the `›` prompt before the draft and the model/effort bar inside. Key hints live in the footer.
import { CURSOR_MARKER, renderFakeCursor, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { splitRow } from "./panels-chrome.ts";
import { pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_LINE, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";
import type { PanelSnapshot } from "./panels-view.ts";

/** A pointer range in cells of the rendered bar, `to` exclusive. */
export interface ComposerHit { readonly from: number; readonly to: number }
/** Cells left of the box's content: the outer margin, the `│` and the inner blank. */
export const COMPOSER_INSET = 3;

/**
 * Where Enter sends a draft Pi runs as an owner shell command, read exactly as Pi reads the submitted text: trimmed, then
 * `!!` (output stays here, out of the agent's context) or `!` (output goes to the agent) before a non-empty command.
 * Undefined for any other draft, including a bare `!`, which Pi sends as a message.
 */
export type ShellDisposition = "agent" | "local";
export function shellDisposition(draft: string): ShellDisposition | undefined {
  const text = draft.trim();
  if (!text.startsWith("!")) return undefined;
  const local = text.startsWith("!!");
  if (!text.slice(local ? 2 : 1).trim()) return undefined;
  return local ? "local" : "agent";
}

/**
 * `Message`, plus the last typed input's disposition while it still matters: pending admission,
 * queued, or refused with its reason. Started and handled input adds nothing. `target` names who
 * the message goes to, in that receiver's colour, when it is not the session's own agent. `shell` replaces the
 * receiver while the draft is an owner `!command`: `→ shell` and where its output goes. Plan mode adds `PLAN`
 * (`PLAN · review` while a submitted plan awaits the owner).
 */
export function composerTitle(snapshot: PanelSnapshot | null,
  target?: { readonly label: string; readonly color: string }, shell?: ShellDisposition): string {
  const input = snapshot?.input;
  const reason = input?.reason ? plainLabel(input.reason) : "";
  const state = input?.disposition === "pending" ? " · last input pending admission"
    : input?.disposition === "queued" ? " · last input queued"
    : input?.disposition === "rejected" ? ` · last input refused${reason ? `: ${reason}` : ""}`
    : "";
  const to = shell ? ` ${PANEL_TEXT}→ shell${PANEL_MUTED} · ${shell === "agent" ? "output goes to the agent" : "output stays here"}`
    : target ? ` ${target.color}→ ${plainLabel(target.label)}${PANEL_MUTED}` : "";
  const plan = snapshot?.planMode ? ` · ${PANEL_BLUE}PLAN${snapshot.planMode === "review" ? " · review" : ""}${PANEL_MUTED}` : "";
  return PANEL_MUTED + "Message" + plan + to + state;
}

function border(focused: boolean): string { return PANEL_BG + (focused ? PANEL_BLUE : PANEL_LINE); }

/**
 * The top border with `title` set into it, the working indicator near the right corner and `right` (the
 * `↑ n more` marker) after it. `status` lists the indicator's variants from the longest to the shortest; the
 * longest that fits is drawn and none when even the shortest would squeeze the title below `keep` cells (the
 * `Message → receiver` part, which survives every tier while the input disposition clips first).
 */
export function composerTop(title: string, width: number, focused: boolean, right = "", status: readonly string[] = [], keep = 0): string {
  const inner = Math.max(0, width - 2), color = border(focused);
  if (inner < 2) return PANEL_BG + " ".repeat(Math.max(0, width));
  const tail = right && inner >= visibleWidth(right) + 12 ? ` ${right} ─` : "";
  const base = inner - 2 - visibleWidth(tail) - 4;
  // The title keeps `keep` cells (or all of itself when shorter); the indicator takes ` <status> ─` from the rest.
  const reserve = title ? Math.min(visibleWidth(title), keep) : 0;
  const shown = status.find(item => base - (visibleWidth(item) + 3) >= reserve) ?? "";
  const mark = shown ? ` ${shown}${color} ─` : "";
  const room = base - visibleWidth(mark);
  const label = title && room > 0 ? truncateToWidth(title, room, "…") : "";
  const head = label ? `─ ${PANEL_TEXT}${label}${color} ` : "─";
  const fill = Math.max(0, inner - 2 - visibleWidth(head) - visibleWidth(mark) - visibleWidth(tail));
  return `${PANEL_BG} ${color}╭${head}${"─".repeat(fill)}${mark}${PANEL_MUTED}${tail.slice(0, -1)}${color}${tail.slice(-1)}╮${PANEL_BG} ${PANEL_TEXT}`;
}
/** The bottom border; in a short terminal it carries `↓ n more` since the bar is gone. */
export function composerBottom(width: number, focused: boolean, right = ""): string {
  const inner = Math.max(0, width - 2), color = border(focused);
  if (inner < 2) return PANEL_BG + " ".repeat(Math.max(0, width));
  const tail = right && inner >= visibleWidth(right) + 6 ? ` ${right} ─` : "";
  return `${PANEL_BG} ${color}╰${"─".repeat(Math.max(0, inner - 2 - visibleWidth(tail)))}${PANEL_MUTED}${tail.slice(0, -1)}${color}${tail.slice(-1)}╯${PANEL_BG} ${PANEL_TEXT}`;
}
/** The zero-width markers Pi's editor draws around its cursor cell, which the TUI turns into reverse video. */
const [FAKE_START = "", FAKE_END = ""] = renderFakeCursor("\u0000").split("\u0000");

/**
 * One editor row inside the box. `row` is the editor's own line, which starts with `left` blank cells of
 * padding and ends with at least two blank cells; the margin, `│` and `prompt` replace the left padding and
 * the last two cells become `│` and the margin, so pointer columns keep their place. Below 12 cells the
 * editor cannot keep that padding and the row stays as it is. `focused` means the composer has the keys:
 * otherwise the `›` is muted and the row carries no cursor (neither the terminal's nor the drawn cell), so
 * the one cursor on screen is where the next key goes.
 */
export function composerRow(row: string, left: number, width: number, focused: boolean, prompt: boolean): string {
  if (width < 12 || left < COMPOSER_INSET) return row;
  const lead = left - COMPOSER_INSET;
  const mark = prompt ? (focused ? PANEL_BLUE : PANEL_MUTED) + "›" + PANEL_TEXT : " ";
  const prefix = `${PANEL_BG} ${border(focused)}│${PANEL_TEXT} ${lead > 0 ? mark + " ".repeat(lead - 1) : ""}`;
  const text = focused ? row : row.replaceAll(CURSOR_MARKER, "").replaceAll(FAKE_START, "").replaceAll(FAKE_END, "");
  const body = text.slice(left), cut = body.endsWith("  ") ? body.slice(0, -2) : truncateToWidth(body, Math.max(0, width - left - 2), "");
  const fill = " ".repeat(Math.max(0, width - left - 2 - visibleWidth(cut)));
  return `${prefix}${cut}${fill}${border(focused)}│${PANEL_BG} ${PANEL_TEXT}`;
}

/**
 * The bar row inside the box: model ▾ and `· effort <level> ↻` on the left, `↓ n more` on the right when the
 * editor hides lines. The word `effort` goes before the model name would clip. The hit ranges are cells of the
 * full row, from the same measurement that lays the row out.
 */
export function composerBar(snapshot: { readonly model: string | null; readonly effort?: string; readonly effortAvailable?: boolean } | null,
  width: number, hidden: number, focused: boolean, readOnly = false): {
  readonly line: string; readonly model?: ComposerHit; readonly effort?: ComposerHit;
} {
  const full = snapshot?.model, cycle = !readOnly && snapshot?.effortAvailable;
  const model = full ? plainLabel(full.slice(full.indexOf("/") + 1)) + (readOnly ? "" : " ▾") : "Model unavailable";
  const more = hidden > 0 ? `↓ ${hidden} more` : "";
  const inner = Math.max(0, width - 2 * COMPOSER_INSET), moreCells = more ? visibleWidth(more) + 2 : 0;
  const level = snapshot?.effort === undefined ? "" : `${plainLabel(snapshot.effort)}${cycle ? " ↻" : ""}`;
  const worded = level ? ` · effort ${level}` : "";
  const effort = inner - visibleWidth(worded) - moreCells >= visibleWidth(model) ? worded : level ? ` · ${level}` : "";
  const room = Math.max(0, inner - visibleWidth(effort) - moreCells);
  const label = pad(model, Math.min(room, visibleWidth(model))).trimEnd();
  const modelEnd = COMPOSER_INSET + visibleWidth(label), effortEnd = modelEnd + visibleWidth(effort);
  const color = border(focused);
  return {
    line: `${PANEL_BG} ${color}│${PANEL_TEXT} ` + splitRow(PANEL_TEXT + label + PANEL_MUTED + effort, PANEL_MUTED + more, inner) + `${PANEL_BG} ${color}│${PANEL_BG} ${PANEL_TEXT}`,
    ...(!readOnly && full && label ? { model: { from: COMPOSER_INSET, to: modelEnd } } : {}),
    ...(cycle && level && effortEnd <= width - COMPOSER_INSET ? { effort: { from: modelEnd, to: effortEnd } } : {}),
  };
}
