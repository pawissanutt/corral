import { keyText } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import type { ActivityRow } from "./activity-view.ts";
import { plainLabel } from "./panels-view.ts";

export function helperKind(kind: HelperInspection["kind"]): string {
  switch (kind) { case "explore": return "Explore"; case "oracle": return "Oracle"; case "web_search": return "Search"; }
}
export function helperPhase(phase: HelperInspection["phase"]): string {
  switch (phase) {
    case "preparing": return "Preparing";
    case "queued": return "Queued";
    case "running": return "Running";
    case "completed": return "Response finished";
    case "uncertain": return "Interrupted / outcome uncertain";
    case "not_started": return "Not started";
  }
}
/** Selected-agent metadata only; selection lives in the sidebar, rendering in HelperChat/Pi. */
export function helperRows(helpers: readonly HelperInspection[] | null | undefined, selected: string | undefined, bound: boolean): ActivityRow[] {
  const rows: ActivityRow[] = [{ text: "Agent conversation · read-only", color: "accent" },
    { text: `F8 agents & jobs · ${keyText("app.tools.expand")} tools · PgUp/PgDn scroll · F9 conversation`, color: "muted" }];
  if (!bound) rows.push({ text: "Disconnected/unbound · observations may be stale", color: "warning" });
  if (helpers == null) return [...rows, { text: "Helper history unavailable for this host.", color: "warning" }];
  if (!helpers.length) return [...rows, { text: "No helper calls yet." }, { text: "New research and oracle conversations appear here automatically.", color: "muted" }];
  const index = helpers.findIndex(helper => helper.request === selected);
  rows.push({ text: "Current host observation · not task completion or gate evidence", color: "muted" });
  const helper = helpers[index];
  if (!helper) return [...rows, { text: "Selected agent is no longer retained. Press F8 to choose another.", color: "warning" }];
  rows.push({ text: `${helperKind(helper.kind)} · ${helperPhase(helper.phase)}`, color: "accent" });
  rows.push({ text: `${plainLabel(helper.model)}${helper.serviceTier ? " · Fast (priority)" : ""} · g${helper.generation}`, color: "muted" });
  if (helper.truncated) rows.push({ text: "Some content omitted by history/text retention limits.", color: "warning" });
  for (const [label, text] of [["Inspector warning", helper.inspectionError], ["Observed error", helper.error]] as const) {
    if (text) rows.push({ text: label, color: "warning" }, ...stripTerminalSequences(text).split("\n").map(text => ({ text, color: "warning" as const })));
  }
  if (helper.phase === "queued" || helper.phase === "preparing") rows.push({ text: "No provider response yet.", color: "muted" });
  return rows;
}
