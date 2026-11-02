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
export function helperRows(helpers: readonly HelperInspection[] | null | undefined, selected: string | undefined, bound: boolean): ActivityRow[] {
  const rows: ActivityRow[] = [];
  let displayTruncated = false;
  const add = (text: string, color?: ActivityRow["color"]) => {
    if (rows.length < 1000) rows.push({ text, color }); else displayTruncated = true;
  };
  const block = (label: string, text: string, priority = false) => {
    const all = stripTerminalSequences(text).split("\n");
    const lines = all.length > 120 ? [...all.slice(0, 60), `[${all.length - 120} lines omitted]`, ...all.slice(-60)] : all;
    if (all.length > 120) displayTruncated = true;
    const push = priority ? (text: string, color?: ActivityRow["color"]) => { rows.push({ text, color }); } : add;
    push(""); push(label, "accent");
    for (const line of lines) push(line);
  };
  add("Helper conversations · read-only", "accent");
  add("j/k or click selects · PgUp/PgDn scroll · Enter compose · F9 conversation", "muted");
  add("Local observations, not task completion or gate evidence.", "muted");
  if (!bound) add("Disconnected/unbound · observations may be stale", "warning");
  add(`Current host only · retains up to ${HELPER_HISTORY_LIMIT} helpers · private reasoning excluded`, "muted"); add("");
  if (helpers == null) { add("Helper history unavailable for this host.", "warning"); return rows; }
  if (!helpers.length) { add("No helper calls yet."); add("New research and oracle conversations appear here automatically.", "muted"); return rows; }
  for (const helper of helpers) rows.push({
    text: `${helper.request === selected ? "›" : " "} ${helperKind(helper.kind)} ${helper.request.slice(0, 8)} · ${helperPhase(helper.phase)} · ${plainLabel(helper.prompt).slice(0, 70)}`,
    helper: helper.request, color: helper.request === selected ? "accent" : "muted",
  });
  const helper = helpers.find(helper => helper.request === selected);
  if (!helper) { add(""); add("Selected helper is no longer retained. Choose another with j/k or click.", "warning"); return rows; }
  block(`${helperKind(helper.kind)} · ${helperPhase(helper.phase)}`, `${helper.model} · generation ${helper.generation}\nRequest ${helper.request}`);
  if (helper.truncated) add("Some content omitted by history/text retention limits.", "warning");
  if (helper.inspectionError) block("Inspector warning", helper.inspectionError, true);
  if (helper.error) block("Observed error", helper.error, true);
  block("Prompt", helper.prompt);
  for (const message of helper.messages) {
    if (message.kind === "assistant") { if (message.text) block("Assistant", message.text); }
    else {
      block(`Tool · ${message.tool ?? "unavailable"} · ${message.phase ?? "observed"}`, message.arguments ?? "");
      if (message.text) block("Tool result", message.text);
    }
  }
  if (helper.result !== undefined) block("Returned result", helper.result, true);
  else if (helper.phase === "running") add("Waiting for further helper activity…", "muted");
  else if (helper.phase === "queued" || helper.phase === "preparing") add("No provider response yet.", "muted");
  if (displayTruncated) rows.push({ text: "Display truncated: long sections retain their start/end; result and errors remain separate.", color: "warning" });
  return rows;
}
