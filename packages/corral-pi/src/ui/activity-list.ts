// Activity › Now primitives: one entry model for the split list, the narrow picker and j/k selection,
// plus the chip/bar/tone vocabulary the job and agent details share. Colours are the packaged PANEL_* tokens.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { activityKey, type ActivityTarget } from "./activity-sidebar.ts";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_SUCCESS,
  PANEL_TEXT, PANEL_WARNING, RESET, type PanelJob, type PanelSnapshot,
} from "./panels-view.ts";

/** At and above this width the list sits beside the detail; below it collapses to a one-line picker. */
export const SPLIT_MIN_WIDTH = 105;
/** List pane width, including its right rule. */
export const LIST_WIDTH = 42;

export type Tone = "text" | "muted" | "accent" | "success" | "warning" | "error";
export function toneColor(tone: Tone): string {
  switch (tone) {
    case "text": return PANEL_TEXT;
    case "muted": return PANEL_MUTED;
    case "accent": return PANEL_BLUE;
    case "success": return PANEL_SUCCESS;
    case "warning": return PANEL_WARNING;
    case "error": return PANEL_ERROR;
  }
}
export interface Chip { readonly text: string; readonly tone: Tone }
/** A raised label: the selected-row surface with a toned glyph and word, so colour is never the only signal. */
export function chip({ text, tone }: Chip): string {
  return PANEL_SELECTED + toneColor(tone) + " " + text + " " + PANEL_BG + PANEL_TEXT;
}
/** Chips flow left to right after `lead` and wrap to new rows; a chip wider than a row is clipped. */
export function chipRows(chips: readonly Chip[], width: number, lead = ""): string[] {
  const rows: string[] = [];
  let row = lead, used = visibleWidth(lead);
  for (const item of chips) {
    const text = truncateToWidth(item.text, Math.max(1, width - 2), "…"), size = visibleWidth(text) + 2;
    const gap = used === 0 ? 0 : row === lead ? 2 : 1;
    if (used > 0 && used + gap + size > width) { rows.push(row); row = chip({ text, tone: item.tone }); used = size; continue; }
    row += " ".repeat(gap) + chip({ text, tone: item.tone }); used += gap + size;
  }
  if (row) rows.push(row);
  return rows;
}
export function bar(color: string, text: string): string { return color + "▎ " + PANEL_TEXT + text; }

export function duration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h${String(Math.floor(s % 3600 / 60)).padStart(2, "0")}m`;
}
export function ago(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
}

export interface Status { readonly glyph: string; readonly tone: Tone; readonly word: string; readonly time?: string }
export function jobStatus(job: PanelJob, now: number): Status {
  const d = job.detail;
  const time = d?.startedAt === undefined ? undefined : duration((d.endedAt ?? now) - d.startedAt);
  const timed = (status: Omit<Status, "time">): Status => time === undefined ? status : { ...status, time };
  if (job.state === "running") return timed({ glyph: "●", tone: "accent", word: "running" });
  if (job.state !== "exited") return timed({ glyph: "!", tone: "warning", word: plainLabel(job.state) });
  if (d?.signal) return timed({ glyph: "✗", tone: "error", word: plainLabel(d.signal) });
  if (d?.exitCode === 0) return timed({ glyph: "✓", tone: "success", word: "exit 0" });
  if (d?.exitCode != null) return timed({ glyph: "✗", tone: "error", word: `exit ${d.exitCode}` });
  return timed({ glyph: "!", tone: "warning", word: "exit unknown" });
}
export function helperStatus(helper: HelperInspection, now: number): Status {
  const time = helper.startedAt === undefined ? undefined : duration((helper.endedAt ?? now) - helper.startedAt);
  const timed = (status: Omit<Status, "time">): Status => time === undefined ? status : { ...status, time };
  switch (helper.phase) {
    case "running": return timed({ glyph: "●", tone: "accent", word: "running" });
    case "preparing": case "queued": return { glyph: "○", tone: "muted", word: helper.phase };
    case "completed": return helper.error || helper.inspectionError ? timed({ glyph: "!", tone: "warning", word: "failed" })
      : timed({ glyph: "✓", tone: "success", word: "done" });
    case "uncertain": return timed({ glyph: "!", tone: "warning", word: "uncertain" });
    case "not_started": return { glyph: "!", tone: "warning", word: "not started" };
  }
}
export function helperKindName(kind: HelperInspection["kind"]): string {
  switch (kind) { case "explore": return "explore"; case "oracle": return "oracle"; case "web_search": return "search"; }
}

export interface ActivityEntry {
  readonly target: ActivityTarget; readonly key: string;
  readonly glyph: string; readonly tone: Tone; readonly label: string; readonly right: string; readonly rightTone: Tone;
}
/** Jobs, then helpers, in host order: the list, the picker and j/k all walk this one sequence. */
export function activityEntries(snapshot: PanelSnapshot | null, now: number): ActivityEntry[] {
  if (!snapshot) return [];
  const jobs = (snapshot.jobs ?? []).map((job): ActivityEntry => {
    const status = jobStatus(job, now), target = { kind: "job" as const, job: job.job };
    const quiet = status.tone === "accent" || status.tone === "success";
    return { target, key: activityKey(target), glyph: status.glyph, tone: status.tone, label: plainLabel(job.detail?.command ?? job.job),
      right: quiet ? status.time ?? status.word : status.word, rightTone: quiet ? "muted" : status.tone };
  });
  const helpers = (snapshot.helpers ?? []).map((helper): ActivityEntry => {
    const status = helperStatus(helper, now), target = { kind: "helper" as const, request: helper.request };
    const right = status.word === "done" ? helper.endedAt === undefined ? status.time ?? "done" : ago(now - helper.endedAt)
      : status.tone === "accent" ? status.time ?? status.word : status.word;
    return { target, key: activityKey(target), glyph: status.glyph, tone: status.tone,
      label: `${helperKindName(helper.kind)} · ${plainLabel(helper.prompt)}`, right, rightTone: status.tone === "accent" || status.tone === "success" ? "muted" : status.tone };
  });
  return [...jobs, ...helpers];
}
/** The shown selection: the chosen entry while retained, else the first entry; `missing` flags an evicted choice. */
export function resolveSelection(entries: readonly ActivityEntry[], selected: ActivityTarget | undefined): { readonly entry?: ActivityEntry; readonly missing: boolean } {
  if (!selected) return entries[0] ? { entry: entries[0], missing: false } : { missing: false };
  const key = activityKey(selected), entry = entries.find(item => item.key === key);
  return entry ? { entry, missing: false } : { missing: true };
}

function listRow(entry: ActivityEntry, selected: boolean): string {
  const inner = LIST_WIDTH - 1, right = truncateToWidth(entry.right, 14, "…");
  const room = Math.max(1, inner - 4 - visibleWidth(right) - 2);
  const label = truncateToWidth(entry.label, room, "…");
  const gap = " ".repeat(Math.max(1, inner - 3 - visibleWidth(label) - visibleWidth(right) - 1));
  return (selected ? PANEL_SELECTED + PANEL_BLUE + "▌" : PANEL_BG + " ") + toneColor(entry.tone) + entry.glyph + " "
    + PANEL_TEXT + label + gap + toneColor(entry.rightTone) + right + " " + PANEL_BG + PANEL_RULE + "│" + RESET;
}
function listText(text: string, color = PANEL_MUTED): string {
  return PANEL_BG + color + pad("  " + text, LIST_WIDTH - 1) + PANEL_RULE + "│" + RESET;
}
export const LIST_BLANK = PANEL_BG + " ".repeat(LIST_WIDTH - 1) + PANEL_RULE + "│" + RESET;

/** The left pane; `hits[row]` is the entry a click on that row selects. */
export function listPane(snapshot: PanelSnapshot, entries: readonly ActivityEntry[], selectedKey: string | undefined): { readonly lines: string[]; readonly hits: (ActivityTarget | undefined)[] } {
  const lines: string[] = [], hits: (ActivityTarget | undefined)[] = [];
  const push = (line: string, target?: ActivityTarget) => { lines.push(line); hits.push(target); };
  const section = (heading: string, kind: "job" | "helper", source: readonly unknown[] | null | undefined, empty: string) => {
    push(listText(heading));
    if (source == null) return;
    const rows = entries.filter(entry => entry.target.kind === kind);
    if (!rows.length) push(listText(empty));
    for (const entry of rows) push(listRow(entry, entry.key === selectedKey), entry.target);
  };
  const jobs = snapshot.jobs, helpers = snapshot.helpers;
  section(jobs === null ? "JOBS · unavailable" : `JOBS · ${(jobs ?? []).filter(job => job.state === "running").length} running`, "job", jobs, "no background jobs");
  push(LIST_BLANK);
  section(helpers == null ? "AGENTS · unavailable" : `AGENTS · ${helpers.filter(helper => ["preparing", "queued", "running"].includes(helper.phase)).length} active`,
    "helper", helpers, "no helper calls yet");
  return { lines, hits };
}
/** Sources the host could not read, so an empty list is not a claim that nothing ran. */
export function unavailableSources(snapshot: PanelSnapshot | null): string[] {
  return snapshot ? [...(snapshot.jobs === null ? ["jobs"] : []), ...(snapshot.helpers == null ? ["helpers"] : [])] : [];
}
export function pickerRow(entries: readonly ActivityEntry[], selected: ActivityEntry | undefined, width: number, unavailable: readonly string[] = []): string {
  if (!selected) {
    const text = unavailable.length === 2 ? " ! Jobs and helpers unavailable" : unavailable.length ? ` ! ${unavailable[0] === "jobs" ? "Jobs" : "Helpers"} unavailable · no ${unavailable[0] === "jobs" ? "helpers" : "jobs"} yet` : " No jobs or helpers yet";
    return PANEL_BG + (unavailable.length ? PANEL_WARNING : PANEL_MUTED) + pad(text, width) + RESET;
  }
  const index = entries.findIndex(entry => entry.key === selected.key);
  const right = toneColor(selected.rightTone) + selected.right + PANEL_MUTED + `  ‹ ${index + 1}/${entries.length} ›`;
  return PANEL_SELECTED + splitRow(PANEL_BLUE + "▌" + toneColor(selected.tone) + selected.glyph + " " + PANEL_TEXT + selected.label, right, width) + RESET;
}

const HINTS = {
  job: [["j/k", "select"], ["p", "Peek"], ["b", "Background"], ["x", "Stop"], ["f", "follow on/off"], ["Enter", "output pager"], ["/", "search output"], ["?", "legend"], ["Esc", "chat"]],
  helper: [["j/k", "select"], ["Enter", "expand tool calls"], ["t", "open transcript (read-only)"], ["?", "legend"], ["Esc", "chat"]],
} as const;
/** The pinned key-hint row; narrower widths shorten descriptions, then keep keys only, so every key stays listed. */
export function activityHints(kind: ActivityTarget["kind"] | undefined, width: number): string {
  const hints = HINTS[kind ?? "job"];
  const tiers = [hints.map(([key, what]) => `${key} ${what}`).join(" · "),
    hints.map(([key, what]) => `${key} ${what.split(" ")[0]}`).join(" · "), hints.map(([key]) => key).join(" · ")];
  const text = tiers.find(tier => visibleWidth(tier) + 2 <= width) ?? tiers[2] ?? "";
  return PANEL_BG + PANEL_MUTED + pad("  " + text, width) + RESET;
}
/** Right chip of the sub-tab row: where these observations come from; a stale host is data, shown inline. */
export function activityScope(snapshot: PanelSnapshot | null, kind: ActivityTarget["kind"] | undefined): string {
  if (!snapshot?.bound) return chip({ text: "stale · unbound", tone: "warning" });
  return chip({ text: kind === "helper" ? "host observations" : "this host only", tone: "text" });
}
