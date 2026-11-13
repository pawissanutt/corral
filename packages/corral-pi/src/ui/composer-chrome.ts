// The composer's own chrome: a rounded box one cell in from each terminal edge, its title set into the top
// border, the `›` prompt before the draft and the model/effort bar inside. Key hints live in the footer.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { splitRow } from "./panels-chrome.ts";
import { pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_LINE, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";
import type { PanelSnapshot } from "./panels-view.ts";

/** A pointer range in cells of the rendered bar, `to` exclusive. */
export interface ComposerHit { readonly from: number; readonly to: number }
/** Cells left of the box's content: the outer margin, the `│` and the inner blank. */
export const COMPOSER_INSET = 3;

/**
 * `Message`, plus the last typed input's disposition while it still matters: pending admission,
 * queued, or refused with its reason. Started and handled input adds nothing. `target` names who
 * the message goes to, in that receiver's colour, when it is not the session's own agent.
 */
export function composerTitle(snapshot: PanelSnapshot | null,
  target?: { readonly label: string; readonly color: string }): string {
  const input = snapshot?.input;
  const reason = input?.reason ? plainLabel(input.reason) : "";
  const state = input?.disposition === "pending" ? " · last input pending admission"
    : input?.disposition === "queued" ? " · last input queued"
    : input?.disposition === "rejected" ? ` · last input refused${reason ? `: ${reason}` : ""}`
    : "";
  const to = target ? ` ${target.color}→ ${plainLabel(target.label)}${PANEL_MUTED}` : "";
  return PANEL_MUTED + "Message" + to + state;
}

function border(focused: boolean): string { return PANEL_BG + (focused ? PANEL_BLUE : PANEL_LINE); }

/** The top border with `title` set into it and `right` (the `↑ n more` marker) near its right corner. */
export function composerTop(title: string, width: number, focused: boolean, right = ""): string {
  const inner = Math.max(0, width - 2), color = border(focused);
  if (inner < 2) return PANEL_BG + " ".repeat(Math.max(0, width));
  const tail = right && inner >= visibleWidth(right) + 12 ? ` ${right} ─` : "";
  const room = inner - 2 - visibleWidth(tail) - 4;
  const label = title && room > 0 ? truncateToWidth(title, room, "…") : "";
  const head = label ? `─ ${PANEL_TEXT}${label}${color} ` : "─";
  const fill = Math.max(0, inner - 2 - visibleWidth(head) - visibleWidth(tail));
  return `${PANEL_BG} ${color}╭${head}${"─".repeat(fill)}${PANEL_MUTED}${tail.slice(0, -1)}${color}${tail.slice(-1)}╮${PANEL_BG} ${PANEL_TEXT}`;
}
/** The bottom border; in a short terminal it carries `↓ n more` since the bar is gone. */
export function composerBottom(width: number, focused: boolean, right = ""): string {
  const inner = Math.max(0, width - 2), color = border(focused);
  if (inner < 2) return PANEL_BG + " ".repeat(Math.max(0, width));
  const tail = right && inner >= visibleWidth(right) + 6 ? ` ${right} ─` : "";
  return `${PANEL_BG} ${color}╰${"─".repeat(Math.max(0, inner - 2 - visibleWidth(tail)))}${PANEL_MUTED}${tail.slice(0, -1)}${color}${tail.slice(-1)}╯${PANEL_BG} ${PANEL_TEXT}`;
}
/**
 * One editor row inside the box. `row` is the editor's own line, which starts with `left` blank cells of
 * padding and ends with at least two blank cells; the margin, `│` and `prompt` replace the left padding and
 * the last two cells become `│` and the margin, so pointer columns keep their place. Below 12 cells the
 * editor cannot keep that padding and the row stays as it is.
 */
export function composerRow(row: string, left: number, width: number, focused: boolean, prompt: boolean): string {
  if (width < 12 || left < COMPOSER_INSET) return row;
  const lead = left - COMPOSER_INSET;
  const mark = prompt ? PANEL_BLUE + "›" + PANEL_TEXT : " ";
  const prefix = `${PANEL_BG} ${border(focused)}│${PANEL_TEXT} ${lead > 0 ? mark + " ".repeat(lead - 1) : ""}`;
  const body = row.slice(left), cut = body.endsWith("  ") ? body.slice(0, -2) : truncateToWidth(body, Math.max(0, width - left - 2), "");
  const fill = " ".repeat(Math.max(0, width - left - 2 - visibleWidth(cut)));
  return `${prefix}${cut}${fill}${border(focused)}│${PANEL_BG} ${PANEL_TEXT}`;
}

/**
 * The bar row inside the box: model ▾ and effort ↻ on the left, `↓ n more` on the right when the editor hides
 * lines. The hit ranges are cells of the full row, from the same measurement that lays the row out.
 */
export function composerBar(snapshot: PanelSnapshot | null, width: number, hidden: number, focused: boolean): {
  readonly line: string; readonly model?: ComposerHit; readonly effort?: ComposerHit;
} {
  const full = snapshot?.model;
  const model = full ? plainLabel(full.slice(full.indexOf("/") + 1)) + " ▾" : "Model unavailable";
  const effort = snapshot ? ` · ${plainLabel(snapshot.effort)}${snapshot.effortAvailable ? " ↻" : ""}` : "";
  const more = hidden > 0 ? `↓ ${hidden} more` : "";
  const inner = Math.max(0, width - 2 * COMPOSER_INSET);
  const room = Math.max(0, inner - visibleWidth(effort) - (more ? visibleWidth(more) + 2 : 0));
  const label = pad(model, Math.min(room, visibleWidth(model))).trimEnd();
  const modelEnd = COMPOSER_INSET + visibleWidth(label), effortEnd = modelEnd + visibleWidth(effort);
  const color = border(focused);
  return {
    line: `${PANEL_BG} ${color}│${PANEL_TEXT} ` + splitRow(PANEL_TEXT + label + PANEL_MUTED + effort, PANEL_MUTED + more, inner) + `${PANEL_BG} ${color}│${PANEL_BG} ${PANEL_TEXT}`,
    ...(full && label ? { model: { from: COMPOSER_INSET, to: modelEnd } } : {}),
    ...(snapshot?.effortAvailable && effortEnd <= width - COMPOSER_INSET ? { effort: { from: modelEnd, to: effortEnd } } : {}),
  };
}
