import { stripTerminalSequences } from "@earendil-works/pi-tui";
import type { PanelSnapshot } from "./panels-view.ts";
import { inspectorActivity } from "./inspector.ts";
export interface ActivityRow { readonly text: string; readonly job?: number; readonly color?: "accent" | "success" | "error" | "warning" }
export function activityRows(snapshot: PanelSnapshot | null, selected: number, now = Date.now()): ActivityRow[] {
  if (!snapshot) return [{ text: "Activity unavailable: session snapshot disconnected." }];
  const rows: ActivityRow[] = [];
  const add = (...texts: string[]) => rows.push(...texts.map(text => ({ text })));
  add("Background activity · read-only", "g dependencies · l lifecycle · a scoped agents · j/k select · PgUp/PgDn scroll · Enter compose · F9 conversation",
    snapshot.bound ? "Bound host observations · not canonical task/run settlement" : "Disconnected/unbound · observations may be stale", "");
  // Keep jobs inspectable before potentially long receipt/event history.
  const finish = () => {
    add("", "Host receipt/event history");
    if (snapshot.inspector) add(...inspectorActivity(snapshot.inspector));
    else add("Receipt/event history unavailable · legacy snapshot");
    return rows;
  };
  const auxiliary = snapshot.auxiliary;
  add(auxiliary ? `Helper slots occupied / queued: research ${auxiliary.explore.running}/${auxiliary.explore.queued} · oracle ${auxiliary.oracle.running}/${auxiliary.oracle.queued}` : "Helper telemetry unavailable");
  add("Board task/run monitoring is separate; no task completion inferred here.", "");
  if (snapshot.jobs === null) { add("Background-job runtime unavailable for this role."); return finish(); }
  if (!snapshot.jobs.length) { add("No background jobs in this host.", "Jobs started by the agent appear here automatically."); return finish(); }
  const lost = snapshot.jobs.filter(job => job.state === "lost").length;
  add(`${snapshot.jobs.filter(job => job.state === "running").length} running · ${snapshot.jobs.filter(job => job.state === "exited").length} exited · ${lost ? lost + " lost" : "lost status unavailable"}`,
    "Source: this host runtime only; missing jobs are not completed. No stop/restart controls.",
    "Readiness unavailable unless a retained line match is observed; a match is not service health.", "");
  snapshot.jobs.forEach((job, index) => {
    const d = job.detail;
    const elapsed = d?.startedAt === undefined ? "elapsed unavailable" : `${Math.max(0, Math.floor(((d.endedAt ?? now) - d.startedAt) / 1000))}s elapsed`;
    const outcome = d?.state === "exited" ? ` · exit ${d.exitCode ?? "unknown"}${d.signal ? " · " + d.signal : ""}` : "";
    rows.push({ text: `${index === selected ? ">" : " "} ${job.job} · ${job.state}${outcome} · ${elapsed}`, job: index,
      color: job.state === "running" ? "accent" : d?.exitCode === 0 ? "success" : d?.signal || d?.exitCode != null ? "error" : "warning" });
    if (d) add(`  [${d.startedAt === undefined ? "not launched" : "launched"}] → [${d.state}] → [${d.groupClear ? "group clear" : "group cleanup unconfirmed"}]`,
      "  " + stripTerminalSequences(d.command).replace(/[\u0000-\u001f\u007f-\u009f]/g, " "));
  });
  const job = snapshot.jobs[selected]; if (!snapshot.bound || !job?.detail || job.state === "lost") {
    add("", !snapshot.bound ? "Output unavailable: stale/disconnected host" : !job ? "Selected job missing / observation lost" : "Detailed output unavailable for this job source."); return finish();
  }
  const d = job.detail;
  add("", `${d.job} · ${d.bytes} output bytes${d.truncated ? " · stored output truncated" : ""}`, `Process group ${d.groupClear ? "observed clear" : "not confirmed clear"}; exit does not imply gate PASS.`, "", `Monitoring · ${d.monitorAttempts} attempts (not active-wait count)`);
  if (!d.watches.length) add("No retained line-monitor observations. Exit waits/settlement require canonical receipts.");
  for (const w of d.watches) {
    const state = w.matched !== undefined ? "match observed" : Date.parse(w.deadline) < now ? "deadline elapsed · outcome unknown" : w.armed ? "armed locally" : "registering";
    add(`${w.wait} /${w.pattern}/ · ${state}${w.reportQueued ? " · report queued, settlement unknown" : ""}`, `Deadline ${w.deadline}`);
    if (w.matched !== undefined) add("Matched: " + w.matched);
  }
  add("", "Recent captured output (bounded tail)", ...stripTerminalSequences(d.tail).split("\n").slice(-120));
  return finish();
}
export function activityLines(snapshot: PanelSnapshot | null, selected: number, now = Date.now()): string[] {
  return activityRows(snapshot, selected, now).map(row => row.text);
}
