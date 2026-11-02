import { keyText } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
import { HELPER_HISTORY_LIMIT, type HelperInspection } from "../extensions/subagent/inspection.ts";
import type { ActivityRow } from "./activity-view.ts";
import { plainLabel } from "./panels-view.ts";

export function helperKind(kind: HelperInspection["kind"]): string {
  switch (kind) { case "explore": return "Research"; case "oracle": return "Oracle"; case "web_search": return "Search"; }
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
/** Selector and observation metadata only; conversation rendering belongs to HelperChat/Pi. */
export function helperRows(helpers: readonly HelperInspection[] | null | undefined, selected: string | undefined, bound: boolean): ActivityRow[] {
  const rows: ActivityRow[] = [{ text: "Helper conversations · read-only", color: "accent" },
    { text: `j/k select · ${keyText("app.tools.expand")} tools · PgUp/PgDn scroll · Enter compose · F9 conversation`, color: "muted" }];
  if (!bound) rows.push({ text: "Disconnected/unbound · observations may be stale", color: "warning" });
  if (helpers == null) return [...rows, { text: "Helper history unavailable for this host.", color: "warning" }];
  if (!helpers.length) return [...rows, { text: "No helper calls yet." }, { text: "New research and oracle conversations appear here automatically.", color: "muted" }];
  const index = helpers.findIndex(helper => helper.request === selected);
  const start = Math.max(0, Math.min(index - 1, helpers.length - 3));
  rows.push({ text: `Current host · ${helpers.length}/${HELPER_HISTORY_LIMIT} retained · not task completion or gate evidence`, color: "muted" });
  if (helpers.length > 3) rows.push({ text: `${index + 1 || "–"}/${helpers.length} · j/k browses all helpers`, color: "muted" });
  for (const helper of helpers.slice(start, start + 3)) rows.push({
    text: `${helper.request === selected ? "›" : " "} ${helperKind(helper.kind)} ${helper.request.slice(0, 8)} · ${helperPhase(helper.phase)} · ${plainLabel(helper.prompt).slice(0, 55)}`,
    helper: helper.request, color: helper.request === selected ? "accent" : "muted",
  });
  const helper = helpers[index];
  if (!helper) return [...rows, { text: "Selected helper is no longer retained. Choose another with j/k or click.", color: "warning" }];
  rows.push({ text: `${plainLabel(helper.model)}${helper.serviceTier ? " · Fast (priority)" : ""} · g${helper.generation}`, color: "muted" });
  if (helper.truncated) rows.push({ text: "Some content omitted by history/text retention limits.", color: "warning" });
  for (const [label, text] of [["Inspector warning", helper.inspectionError], ["Observed error", helper.error]] as const) {
    if (text) rows.push({ text: label, color: "warning" }, ...stripTerminalSequences(text).split("\n").map(text => ({ text, color: "warning" as const })));
  }
  if (helper.phase === "queued" || helper.phase === "preparing") rows.push({ text: "No provider response yet.", color: "muted" });
  return rows;
}
