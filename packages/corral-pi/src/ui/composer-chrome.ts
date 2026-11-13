// The composer's own chrome: its title and the model/effort bar. Key hints live only in the footer.
import { visibleWidth } from "@earendil-works/pi-tui";
import { splitRow } from "./panels-chrome.ts";
import { pad, plainLabel, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";
import type { PanelSnapshot } from "./panels-view.ts";

/** A pointer range in cells of the rendered bar, `to` exclusive. */
export interface ComposerHit { readonly from: number; readonly to: number }

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
  return "Message" + to + state;
}

/**
 * Model ▾ and effort ↻ on the left, `↓ n more` on the right when the editor hides lines. The hit
 * ranges come from the same measurement that lays the row out. `short` gives a plain rule and no hits.
 */
export function composerBar(snapshot: PanelSnapshot | null, width: number, hidden: number, short: boolean): {
  readonly line: string; readonly model?: ComposerHit; readonly effort?: ComposerHit;
} {
  if (short) return { line: PANEL_MUTED + "─".repeat(Math.max(0, width)) };
  const full = snapshot?.model;
  const model = full ? plainLabel(full.slice(full.indexOf("/") + 1)) + " ▾" : "Model unavailable";
  const effort = snapshot ? ` · ${plainLabel(snapshot.effort)}${snapshot.effortAvailable ? " ↻" : ""}` : "";
  const more = hidden > 0 ? `↓ ${hidden} more` : "";
  const inner = Math.max(0, width - 4);
  const room = Math.max(0, inner - visibleWidth(effort) - (more ? visibleWidth(more) + 2 : 0));
  const label = pad(model, Math.min(room, visibleWidth(model))).trimEnd();
  const modelEnd = 2 + visibleWidth(label), effortEnd = modelEnd + visibleWidth(effort);
  return {
    line: PANEL_MUTED + "  " + PANEL_TEXT + splitRow(label + PANEL_MUTED + effort, PANEL_MUTED + more, inner) + PANEL_MUTED + "  ",
    ...(full && label ? { model: { from: 2, to: modelEnd } } : {}),
    ...(snapshot?.effortAvailable && effortEnd <= width - 2 ? { effort: { from: modelEnd, to: effortEnd } } : {}),
  };
}
