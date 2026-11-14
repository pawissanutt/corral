// The `/resume` chooser: the workspace's assistant sessions on the shared popup list. Only a stopped
// session can be chosen; the current, live and ended ones stay listed with the reason they cannot be.
// Choosing resumes nothing by itself: AssistantResume revalidates the pick before it writes a request.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Availability, Chooser, ResumeRow } from "../host/assistant-resume.ts";
import { popupPlacement } from "./popup-placement.ts";
import { pad, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { PopupList, type PopupRow } from "./popup-list.ts";

/** What each availability says on its row, in words. */
const WORDS: Record<Availability, string> = {
  current: "current · this pane", live: "! open in another pane", ended: "ended", stopped: "stopped · resumable",
};
/** Words the search also matches; `!` is a mark, not a word. */
const SEARCH: Record<Availability, string> = { current: "current this pane", live: "live open another pane", ended: "ended", stopped: "stopped resumable" };
/** Narrowest title kept on the row's own line; below it the availability and date take a second line. */
const MIN_TITLE = 8;

/** The date as `11-12 10:00`; the exact record is under the selected row. */
function stamp(row: ResumeRow): string {
  const parts = /^\d{4}-(\d\d-\d\d)[T ](\d\d:\d\d)/.exec(row.lastActivity);
  return parts ? `${parts[1]} ${parts[2]}` : truncateToWidth(row.lastActivity || "unknown", 16, "…");
}

/** Why a row cannot be chosen, or undefined when it can; shown on the row, never a grant. */
function reason(row: ResumeRow): string | undefined {
  switch (row.availability) {
    case "current": return "this is the current conversation";
    case "live": return `its host runs in another pane; end it with corral assistant end ${row.id}`;
    case "ended": return "ended sessions stay ended; /new opens a new assistant";
    case "stopped": return undefined;
  }
}

function popupRow(row: ResumeRow, idWidth: number): PopupRow<ResumeRow> {
  const blocked = reason(row);
  const disabled = blocked === undefined ? {} : { disabled: blocked };
  return {
    value: row,
    match: `${row.id} ${row.title} ${row.lastActivity} ${stamp(row)} ${SEARCH[row.availability]}`,
    ...disabled,
    lines(width, selected) {
      const base = blocked === undefined ? PANEL_TEXT : PANEL_MUTED;
      const tone = row.availability === "live" ? PANEL_WARNING : blocked === undefined ? PANEL_TEXT : PANEL_MUTED;
      const lead = `${pad(row.id, idWidth)}  `, indent = " ".repeat(idWidth + 2);
      const state = `${tone}${WORDS[row.availability]}${PANEL_MUTED}  ${stamp(row)}${PANEL_TEXT}`;
      const room = width - visibleWidth(lead) - visibleWidth(state) - 2;
      const together = room >= MIN_TITLE;
      const shown = together ? room : width - visibleWidth(lead);
      const title = visibleWidth(row.title) <= shown ? row.title : truncateToWidth(row.title, Math.max(0, shown), "…");
      const lines = together
        ? [`${base}${lead}${pad(title, room)}${PANEL_TEXT}  ${state}`]
        : [`${base}${lead}${title}${PANEL_TEXT}`, `${indent}${state}`];
      if (!selected) return lines.map(line => truncateToWidth(line, Math.max(1, width), "…"));
      // The selected row carries what the line had to cut: the whole title and the exact record time.
      const detail = Math.max(1, width - indent.length);
      const full = title === row.title ? [] : wrapTextWithAnsi(row.title, detail);
      const when = row.lastActivity ? `last activity ${row.lastActivity}` : "no activity time recorded";
      return [...lines, ...full.map(text => `${PANEL_MUTED}${indent}${text}${PANEL_TEXT}`), `${PANEL_MUTED}${indent}${when}${PANEL_TEXT}`]
        .map(line => truncateToWidth(line, Math.max(1, width), "…"));
    },
  };
}

/** The picker's rows: stopped sessions can be chosen, the others stay listed with the reason they cannot. */
export function resumePopupRows(rows: readonly ResumeRow[]): PopupRow<ResumeRow>[] {
  const idWidth = Math.max(0, ...rows.map(row => visibleWidth(row.id)));
  return rows.map(row => popupRow(row, idWidth));
}

/** A searchable popup over the sessions: ↑↓ move, Enter or a second click picks a stopped one, Esc cancels. */
export const chooseResume: Chooser = (ui, rows) => {
  const entries = resumePopupRows(rows);
  const resumable = rows.filter(row => row.availability === "stopped").length;
  let terminal: { readonly columns: number; readonly rows: number } | undefined;
  return ui.custom<ResumeRow | undefined>((tui, _theme, _keys, done) => {
    terminal = tui.terminal;
    return new PopupList<ResumeRow>({
      title: "Resume an assistant session", right: () => `${resumable} resumable`, placeholder: "search assistant sessions",
      note: "The current conversation stays until you choose.", verb: "resume", empty: "no session matches",
      rows: () => entries,
      capacity: () => Math.max(3, Math.min(14, tui.terminal.rows - 8)),
      done, changed: () => tui.requestRender(),
    });
  }, { overlay: true, overlayOptions: () => popupPlacement(terminal) });
};
