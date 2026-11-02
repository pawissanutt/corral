import { truncateToWidth } from "@earendil-works/pi-tui";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { plainLabel, type PanelLine, type PanelSnapshot, type PanelJob } from "./panels-view.ts";
import { helperKind, helperPhase } from "./helper-view.ts";

export type ActivityTarget = { readonly kind: "helper"; readonly request: string } | { readonly kind: "job"; readonly job: string };
export function activityKey(target: ActivityTarget): string {
  return target.kind === "helper" ? `helper:${target.request}` : `job:${target.job}`;
}
export function helperActive(helper: HelperInspection): boolean {
  return helper.phase === "preparing" || helper.phase === "queued" || helper.phase === "running";
}
function helperCompleted(helper: HelperInspection): boolean {
  return helper.phase === "completed" && !helper.error && !helper.inspectionError;
}
function jobCompleted(job: PanelJob): boolean {
  return job.state === "exited" && job.detail?.exitCode === 0 && !job.detail.signal;
}
function jobLabel(job: PanelJob): string { return plainLabel(job.detail?.command ?? job.job); }
export function activityTitle(snapshot: PanelSnapshot | null, target: ActivityTarget): string {
  if (target.kind === "job") return `Job · ${jobLabel(snapshot?.jobs?.find(job => job.job === target.job) ?? { job: target.job, state: "unknown" })}`;
  const helper = snapshot?.helpers?.find(helper => helper.request === target.request);
  return helper ? `${helperKind(helper.kind)} · ${plainLabel(helper.prompt)}` : "Agent no longer retained";
}

/** One shared selector for the overview, keyboard rail and compact drawer. IDs, never list indices. */
export function activitySidebar(snapshot: PanelSnapshot | null, selected?: string, history = false, now = Date.now()): PanelLine[] {
  if (!snapshot) return [{ text: "Activity unavailable", tone: "warning" }];
  const rows: PanelLine[] = [];
  if (!snapshot.bound) rows.push({ text: "Disconnected/unbound · observations may be stale", tone: "warning" });
  const helpers = snapshot.helpers ?? [], jobs = snapshot.jobs ?? [];
  const completed = helpers.filter(helperCompleted).length + jobs.filter(jobCompleted).length;
  rows.push({ text: snapshot.helpers == null ? "Agents" : `Agents · ${helpers.filter(helperActive).length} active`, heading: true });
  if (snapshot.helpers == null) rows.push({ text: "Agent history unavailable", tone: "muted" });
  else if (!helpers.length) rows.push({ text: "No agents yet", tone: "muted" });
  for (const helper of helpers.filter(helper => history || !helperCompleted(helper))) {
    const action = { kind: "helper" as const, request: helper.request }, chosen = selected === activityKey(action);
    const mark = chosen ? "›" : helperActive(helper) ? "●" : helper.phase === "completed" ? "✓" : "!";
    rows.push({ text: `${mark} ${helperKind(helper.kind)} · ${truncateToWidth(plainLabel(helper.prompt), 24, "…")}`,
      continuationIndent: 2, tone: chosen ? "accent" : helper.phase === "uncertain" ? "warning" : "text", action },
    { text: helperPhase(helper.phase), indent: 2, tone: "muted", action });
  }
  if (helpers.length && !history && helpers.every(helperCompleted)) rows.push({ text: "All agents finished", tone: "muted" });
  rows.push({ text: "" }, { text: snapshot.jobs === null ? "Jobs" : `Jobs · ${jobs.filter(job => job.state === "running").length} running`, heading: true });
  if (snapshot.jobs === null) rows.push({ text: "Background jobs unavailable", tone: "muted" });
  else if (!jobs.length) rows.push({ text: "No background jobs", tone: "muted" });
  for (const job of jobs.filter(job => history || !jobCompleted(job))) {
    const action = { kind: "job" as const, job: job.job }, chosen = selected === activityKey(action), d = job.detail;
    const elapsed = d?.startedAt === undefined ? "" : ` · ${Math.max(0, Math.floor(((d.endedAt ?? now) - d.startedAt) / 1000))}s`;
    const outcome = job.state === "exited" ? ` · exit ${d?.exitCode ?? "unknown"}` : "";
    rows.push({ text: `${chosen ? "›" : job.state === "running" ? "●" : jobCompleted(job) ? "✓" : "!"} ${truncateToWidth(jobLabel(job), 28, "…")}`,
      continuationIndent: 2, tone: chosen ? "accent" : job.state === "exited" && !jobCompleted(job) ? "warning" : "text", action },
    { text: `${plainLabel(job.state)}${outcome}${elapsed}`, indent: 2, tone: "muted", action });
  }
  if (jobs.length && !history && jobs.every(jobCompleted)) rows.push({ text: "All jobs finished", tone: "muted" });
  if (completed) rows.push({ text: "" }, { text: `${history ? "▾" : "▸"} Completed (${completed})`, tone: "accent", action: { kind: "history" } });
  return rows;
}
