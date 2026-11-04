import { stripTerminalSequences } from "@earendil-works/pi-tui";
import type { PanelSnapshot } from "./panels-view.ts";
import { helperRows } from "./helper-view.ts";
import { inspectorActivity } from "./inspector.ts";
export type ActivityMode = "jobs" | "helpers";
export interface ActivityRow { readonly text: string; readonly color?: "accent" | "success" | "error" | "warning" | "text" | "muted" }
/** `legend`: the ? key shows source caveats; unbound/stale states always stay inline. */
export function activityRows(snapshot: PanelSnapshot | null, selected: number, now = Date.now(), mode: ActivityMode = "jobs", helper?: string, legend = false): ActivityRow[] {
  if (!snapshot) return [{ text: "Activity unavailable: session snapshot disconnected." }];
  if (mode === "helpers") return helperRows(snapshot.helpers, helper, snapshot.bound);
  const rows: ActivityRow[] = [{ text: "Job output · read-only", color: "accent" },
    { text: "F8 agents & jobs · PgUp/PgDn scroll · F9 conversation", color: "muted" }];
  const add = (...texts: string[]) => rows.push(...texts.map(text => ({ text })));
  add("Background activity · read-only", `a scoped agents · j/k scroll · ? ${legend ? "hide" : "show"} legend · F9 conversation`);
  if (!snapshot.bound) rows.push({ text: "Disconnected/unbound · observations may be stale", color: "warning" });
  else if (legend) add("Bound host observations · not canonical task/run settlement");
  add("");
  // Keep jobs inspectable before potentially long receipt/event history.
  const finish = () => {
    add("", "Host receipt/event history");
    if (snapshot.inspector) add(...inspectorActivity(snapshot.inspector));
    else add("Receipt/event history unavailable · legacy snapshot");
    return rows;
  };
  const auxiliary = snapshot.auxiliary;
  add(auxiliary ? `Helper slots occupied / queued: research ${auxiliary.explore.running}/${auxiliary.explore.queued} · oracle ${auxiliary.oracle.running}/${auxiliary.oracle.queued}` : "Helper telemetry unavailable");
  if (legend) add("Board task/run monitoring is separate; no task completion inferred here.");
  add("");
  if (snapshot.jobs === null) { add("Background-job runtime unavailable for this role."); return finish(); }
  if (!snapshot.jobs.length) { add("No background jobs in this host.", "Jobs started by the agent appear here automatically."); return finish(); }
  if (!snapshot.bound) { rows.push({ text: "Output unavailable: stale/disconnected host", color: "warning" }); return finish(); }
  const lost = snapshot.jobs.filter(job => job.state === "lost").length;
  add(`${snapshot.jobs.filter(job => job.state === "running").length} running · ${snapshot.jobs.filter(job => job.state === "exited").length} exited · ${lost ? lost + " lost" : "lost status unavailable"}`,
    ...(legend ? ["Source: this host runtime only; missing jobs are not completed. No stop/restart controls.",
      "Readiness unavailable unless a retained line match is observed; a match is not service health."] : []));
  const job = snapshot.jobs[selected];
  if (!job) { add("Selected job missing / no longer retained. Press F8 to choose another."); return finish(); }
  if (job.state === "lost") { add("Detailed output unavailable for this job source."); return finish(); }
  const d = job.detail;
  const elapsed = d?.startedAt === undefined ? "elapsed unavailable" : `${Math.max(0, Math.floor(((d.endedAt ?? now) - d.startedAt) / 1000))}s elapsed`;
  const outcome = job.state === "exited" ? ` · exit ${d?.exitCode ?? "unknown"}${d?.signal ? " · " + d.signal : ""}` : "";
  rows.push({ text: `${job.job} · ${job.state}${outcome} · ${elapsed}`, color: job.state === "running" ? "accent"
    : d?.exitCode === 0 ? "success" : "warning" });
  if (!d) { add("Detailed output unavailable."); return finish(); }
  add(stripTerminalSequences(d.command), "", `${d.bytes} output bytes${d.truncated ? " · stored output truncated" : ""}`,
    `Process group ${d.groupClear ? "observed clear" : "not confirmed clear"}; exit does not imply gate PASS.`, "", "Recent captured output (bounded tail)",
    ...stripTerminalSequences(d.tail).split("\n").slice(-120), "", `Monitoring · ${d.monitorAttempts} attempts (not active-wait count)`);
  if (!d.watches.length) add("No retained line-monitor observations. Exit waits/settlement require canonical receipts.");
  for (const w of d.watches) {
    const state = w.matched !== undefined ? "match observed" : Date.parse(w.deadline) < now ? "deadline elapsed · outcome unknown" : w.armed ? "armed locally" : "registering";
    add(`${w.wait} /${w.pattern}/ · ${state}${w.reportQueued ? " · report queued, settlement unknown" : ""}`, `Deadline ${w.deadline}`);
    if (w.matched !== undefined) add("Matched: " + w.matched);
  }
  return finish();
}
export function activityLines(snapshot: PanelSnapshot | null, selected: number, now = Date.now(), legend = false): string[] {
  return activityRows(snapshot, selected, now, "jobs", undefined, legend).map(row => row.text);
}
