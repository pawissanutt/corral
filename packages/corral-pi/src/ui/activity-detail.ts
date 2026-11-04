// Activity › Now detail panes (mockup §13 job, §14 agent) and the split/picker composition around them.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Markdown, ScrollView, matchesKey, stripTerminalSequences, wrapTextWithAnsi, type MarkdownTheme } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { HELPER_TEXT_LIMIT, type HelperInspection, type HelperMessage } from "../extensions/subagent/inspection.ts";
import type { ActivityTarget } from "./activity-sidebar.ts";
import { helperText } from "./helper-chat.ts";
import {
  activityEntries, bar, chipRows, helperKindName, helperStatus, jobStatus, listPane, pickerRow, resolveSelection,
  LIST_BLANK, LIST_WIDTH, SPLIT_MIN_WIDTH, type ActivityEntry, type Chip,
  unavailableSources,
} from "./activity-list.ts";
import {
  box, pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_RULE, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, RESET,
  type PanelJob, type PanelSnapshot,
} from "./panels-view.ts";

/** Output captured when follow was switched off; the detail keeps showing it until follow resumes. */
export interface JobFreeze { readonly job: string; readonly tail: string; readonly lines: number | undefined; readonly running: boolean }
export interface ActivityViewInput {
  readonly snapshot: PanelSnapshot | null; readonly identity: HostIdentity; readonly selected: ActivityTarget | undefined;
  readonly width: number; readonly height: number; readonly now: number;
  readonly legend: boolean; readonly expanded: boolean; readonly freeze: JobFreeze | undefined; readonly markdownTheme: MarkdownTheme;
}
export interface ActivityView { readonly lines: string[]; readonly hits: ReadonlyMap<number, ActivityTarget>; readonly entry?: ActivityEntry }
export const TOOLS_FOLDED = 3;
/** Layout reports the viewport only after rendering; a changed height re-renders once so the pane fits it. */
export class FittedScrollView extends ScrollView {
  override updateLayout(contentHeight: number, viewportHeight: number, requestRender: () => void): void {
    const resized = Math.max(0, Math.floor(viewportHeight)) !== this.viewportHeight;
    super.updateLayout(contentHeight, viewportHeight, requestRender);
    if (resized) requestRender();
  }
}

function clean(line: string): string {
  const segment = stripTerminalSequences(line.replaceAll("\t", "    ")).split("\r").filter(Boolean).at(-1) ?? "";
  return segment.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
}
/**
 * Numbers the retained tail against the host's finished-line count: while running the last segment is the
 * open line `lines + 1`; once exited the open segment was flushed into `lines`. Unknown counts stay unnumbered.
 */
export function numberedTail(tail: string, lines: number | undefined, running: boolean): { readonly number?: number; readonly text: string }[] {
  let parts = tail.split("\n");
  if (!running && parts.at(-1) === "") parts = parts.slice(0, -1);
  const last = lines === undefined ? undefined : running ? lines + 1 : lines;
  return parts.map((part, index) => {
    const text = clean(part);
    return last === undefined ? { text } : { number: last - (parts.length - 1 - index), text };
  });
}
function outputLine(text: string): string {
  if (/\b(FAILED|panicked)\b|^error(\[|:)/.test(text)) return PANEL_ERROR + text;
  if (/^warning(\[|:)/.test(text)) return PANEL_WARNING + text;
  if (/^\s+(Compiling|Checking|Downloading|Downloaded|Fresh|Blocking)\b/.test(text)) return PANEL_MUTED + text;
  return text.endsWith(" ok") ? PANEL_TEXT + text.slice(0, -2) + PANEL_SUCCESS + "ok" : PANEL_TEXT + text;
}
function size(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
function wrap(text: string, width: number): string[] {
  const rows = text.split("\n").flatMap(line => line.trim() ? wrapTextWithAnsi(line, Math.max(1, width)) : [""]);
  while (rows.at(-1) === "") rows.pop();
  return rows;
}
function legendCard(lines: readonly string[], width: number): string[] {
  const body = lines.flatMap(line => wrap(line, width - 4)).map(line => PANEL_MUTED + line);
  return box("Legend · ? close", body, width, body.length + 2, true);
}

function jobDetail(job: PanelJob, input: ActivityViewInput, width: number, height: number): string[] {
  const d = job.detail, status = jobStatus(job, input.now), bound = input.snapshot?.bound ?? false;
  const cwd = d?.cwd ? " · cwd " + plainLabel(d.cwd.split("/").filter(Boolean).slice(-2).join("/")) : "";
  const head = "\x1b[1m" + PANEL_TEXT + "$ " + plainLabel(d?.command ?? job.job) + "\x1b[22m" + PANEL_MUTED + "   " + plainLabel(job.job) + cwd;
  const chips: Chip[] = [{ text: `${status.glyph} ${status.word}`, tone: status.tone }];
  if (status.time) chips.push({ text: status.time, tone: "text" });
  if (d) chips.push({ text: size(d.bytes), tone: "text" });
  if (d?.pid !== undefined) chips.push(job.state === "exited" && !d.groupClear ? { text: `pg ${d.pid} not clear`, tone: "warning" } : { text: `pg ${d.pid}`, tone: "text" });
  for (const w of d?.watches ?? []) {
    const [state, tone] = w.matched !== undefined ? ["matched", "success"] as const : Date.parse(w.deadline) < input.now ? ["expired", "muted"] as const
      : w.armed ? ["armed", "warning"] as const : ["registering", "muted"] as const;
    chips.push({ text: `⌁ watch /${plainLabel(w.pattern)}/ ${state}`, tone });
  }
  if (!bound) chips.push({ text: "stale", tone: "warning" });
  const cardBody = [head, ...chipRows(chips, width - 4)];
  const rows = box("", cardBody, width, cardBody.length + 2);
  if (input.legend) {
    rows.push(...legendCard(["Source: this host's job runtime only; a job missing here is not completed.",
      "Exit status is a local observation: it does not imply gate PASS or task settlement.",
      ...(d ? [`Process group ${d.groupClear ? "observed clear" : "not confirmed clear"}.`, `Monitoring · ${d.monitorAttempts} attempts (not active-wait count).`] : []),
      ...(d?.watches ?? []).flatMap(w => [`Watch ${plainLabel(w.wait)} /${plainLabel(w.pattern)}/ · deadline ${plainLabel(w.deadline)}${w.reportQueued ? " · report queued, settlement unknown" : ""}`,
        ...(w.matched !== undefined ? ["Matched: " + plainLabel(w.matched)] : [])]),
      "A line match is not service health. There are no stop or restart controls."], width));
  }
  if (job.state === "lost" || !d) return [...rows, PANEL_MUTED + "Detailed output unavailable for this job source."];
  if (!bound) return [...rows, PANEL_WARNING + "Output unavailable: stale/disconnected host"];
  const frozen = input.freeze?.job === job.job ? input.freeze : undefined;
  const source = frozen ?? { tail: d.tail, lines: d.lines, running: job.state === "running" };
  const all = numberedTail(source.tail, source.lines, source.running);
  if (!all.length) return [...rows, PANEL_MUTED + "No output captured yet."];
  const shown = all.slice(-Math.max(3, height - rows.length - 3));
  const last = shown.at(-1)?.number, gutter = String(last ?? "").length;
  const body = shown.map((line, index) => {
    const cursor = !frozen && source.running && index === shown.length - 1 ? PANEL_BLUE + "▍" : "";
    const number = line.number === undefined ? "" : PANEL_MUTED + String(line.number).padStart(gutter) + " " + PANEL_RULE + "│ ";
    return number + (line.text ? outputLine(line.text) : "") + cursor;
  });
  rows.push(...box("", body, width, body.length + 2));
  const count = last === undefined ? `showing last ${shown.length} retained lines` : `showing last ${shown.length} of ${last} lines`;
  const stored = d.truncated ? PANEL_WARNING + "stored output truncated" : "stored output not truncated";
  rows.push(PANEL_MUTED + (frozen ? `‖ follow paused · ${count} at pause · f resume` : source.running ? `▼ following tail · ${count} · ${stored}` : `■ exited · ${count} · ${stored}`));
  return rows;
}

const TOOL_GLYPHS: Readonly<Record<string, string>> = { read: "▤", grep: "⌕", find: "⌕", ls: "⌕", web_search: "⌕", bash: "$", edit: "✎", write: "✎" };
function toolArgs(text: string | undefined): Readonly<Record<string, unknown>> | undefined {
  if (text === undefined) return undefined;
  try {
    const value: unknown = JSON.parse(text);
    return value !== null && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : undefined;
  } catch { return undefined; }
}
function toolLine(message: HelperMessage): string {
  const name = plainLabel(helperText(message.tool ?? "tool")), args = toolArgs(message.arguments);
  const field = (key: string) => { const value = args?.[key]; return typeof value === "string" || typeof value === "number" ? plainLabel(helperText(String(value))) : undefined; };
  const offset = Number(args?.["offset"]), limit = Number(args?.["limit"]);
  const range = Number.isInteger(offset) && offset > 0 ? `:${offset}${Number.isInteger(limit) && limit > 0 ? `-${offset + limit - 1}` : ""}` : "";
  const summary = !args ? plainLabel(helperText(message.arguments ?? "")).slice(0, 80)
    : name === "read" ? (field("path") ?? "") + range
    : name === "grep" ? [field("pattern") === undefined ? "" : `"${field("pattern")}"`, field("path"), field("glob")].filter(Boolean).join(" ")
    : field("command") ?? field("path") ?? field("pattern") ?? field("query") ?? field("url")
      ?? Object.values(args).filter((value): value is string => typeof value === "string").map(value => plainLabel(helperText(value))).join(" ");
  const matches = message.text.split("\n").filter(line => /:\d+:/.test(line)).length;
  const reason = plainLabel(helperText(message.text.split("\n").find(line => line.trim()) ?? "")).trim().slice(0, 80);
  const tail = message.phase === "error" ? PANEL_ERROR + "  · error" + (reason ? `: ${reason}` : "") : message.phase === "running" ? PANEL_BLUE + "  · running"
    : message.phase === "requested" ? PANEL_MUTED + "  · requested"
    : name === "grep" && message.text.length < HELPER_TEXT_LIMIT && (matches || /no matches/i.test(message.text)) ? PANEL_MUTED + `  · ${matches} matches` : "";
  return PANEL_MUTED + (TOOL_GLYPHS[name] ?? "•") + " " + name + "  " + summary + tail;
}
function helperRole(kind: HelperInspection["kind"]): string {
  switch (kind) { case "explore": return "research helper"; case "oracle": return "oracle helper"; case "web_search": return "web search helper"; }
}
function helperDetail(helper: HelperInspection, input: ActivityViewInput, width: number): string[] {
  const snapshot = input.snapshot, status = helperStatus(helper, input.now), agents = snapshot?.agents ?? [];
  const agent = agents.find(item => item.id === helper.request);
  const parent = agent?.parent === undefined ? undefined : agents.find(item => item.session === agent.parent);
  const chips: Chip[] = [{ text: `${status.glyph} ${status.word}${status.time ? " " + status.time : ""}`, tone: status.tone },
    { text: plainLabel(helper.model.slice(helper.model.lastIndexOf("/") + 1)) + (helper.serviceTier ? " · fast" : ""), tone: "text" }];
  if (agent?.tokens !== undefined) chips.push({ text: `${agent.tokens >= 1000 ? `${Number((agent.tokens / 1000).toFixed(1))}k` : agent.tokens} tok`, tone: "text" });
  if (parent) chips.push({ text: `parent: ${plainLabel(parent.kind)}${parent === agents[0] && input.identity.run ? " " + plainLabel(input.identity.run) : ""}`, tone: "text" });
  if (helper.truncated) chips.push({ text: "truncated", tone: "warning" });
  if (!snapshot?.bound) chips.push({ text: "stale", tone: "warning" });
  const title = "\x1b[1m" + PANEL_TEXT + helperKindName(helper.kind) + "\x1b[22m" + PANEL_MUTED + " · " + helperRole(helper.kind) + PANEL_TEXT;
  const rows = chipRows(chips, width, title);
  if (input.legend) rows.push(...legendCard(["Current host observation: not task completion or gate evidence.",
    "Parent links are provenance, not dependency edges. There are no stop or restart controls.",
    `Model ${plainLabel(helper.model)}${helper.serviceTier ? " · Fast (priority)" : ""} · generation g${helper.generation}.`,
    ...(helper.truncated ? ["Some content was omitted by history/text retention limits."] : []),
    snapshot?.bound && agent?.transcript ? "t opens the bounded existing transcript read-only." : "Transcript unavailable for this helper (no eligible session file)."], width));
  const block = (color: string, lines: readonly string[]) => { rows.push(""); rows.push(...lines.map(line => bar(color, line))); };
  const prompt = wrap(helperText(helper.prompt), width - 2), limit = input.expanded ? prompt.length : 6;
  block(PANEL_BLUE, [PANEL_MUTED + "PROMPT", ...prompt.slice(0, limit),
    ...(prompt.length > limit ? [PANEL_MUTED + `… ${prompt.length - limit} more lines (Enter expand)`] : [])]);
  const tools = helper.messages.filter(message => message.kind === "tool");
  if (tools.length) {
    const shown = input.expanded ? tools : tools.slice(0, TOOLS_FOLDED), hidden = tools.length - shown.length;
    block(PANEL_RULE, [...shown.map(toolLine), ...(hidden ? [PANEL_MUTED + `▸ ${hidden} more tool calls  (Enter expand)`]
      : input.expanded && tools.length > TOOLS_FOLDED ? [PANEL_MUTED + "▾ fold tool calls  (Enter)"] : [])]);
  }
  for (const [label, text, color] of [["ERROR", helper.error, PANEL_ERROR], ["INSPECTOR WARNING", helper.inspectionError, PANEL_WARNING]] as const) {
    if (text) block(color, [color + label, ...wrap(helperText(text), width - 2).map(line => color + line)]);
  }
  const last = helper.messages.filter(message => message.kind === "assistant").at(-1);
  const answer = helper.result ?? (last && !last.hasToolCalls ? last.textBlocks?.join("\n\n") ?? last.text : undefined);
  if (answer?.trim()) {
    const streaming = helper.phase === "running" && helper.result === undefined && last?.stopReason === undefined;
    const theme: MarkdownTheme = { ...input.markdownTheme, code: text => PANEL_WARNING + text + PANEL_TEXT };
    const body = new Markdown(helperText(answer), 0, 0, theme).render(Math.max(1, width - 2)).map(line => line.replace(/\s+$/u, ""));
    while (body.at(-1) === "") body.pop();
    if (streaming && body.length) body[body.length - 1] += PANEL_BLUE + "▍";
    block(PANEL_SUCCESS, [PANEL_SUCCESS + "ANSWER" + (streaming ? PANEL_MUTED + " (streaming)" : ""), ...body]);
  } else if (helper.phase === "queued" || helper.phase === "preparing") rows.push("", PANEL_MUTED + "No provider response yet.");
  return rows;
}

function detailLines(input: ActivityViewInput, entry: ActivityEntry | undefined, missing: boolean, width: number, height: number): string[] {
  const snapshot = input.snapshot;
  if (missing) return [PANEL_WARNING + `Selected ${input.selected?.kind === "helper" ? "agent" : "job"} is no longer retained · j/k to choose another`];
  if (!entry) {
    const unavailable = unavailableSources(snapshot);
    if (unavailable.length) return [PANEL_WARNING + `! ${unavailable.join(" and ")} unavailable from the host at read`, PANEL_MUTED + "Unavailable is not empty: nothing is known about what ran."];
    return [PANEL_TEXT + "No background jobs or helper calls yet.", PANEL_MUTED + "Jobs and helpers the agent starts appear here automatically."];
  }
  const target = entry.target;
  if (target.kind === "job") { const job = snapshot?.jobs?.find(item => item.job === target.job); return job ? jobDetail(job, input, width, height) : []; }
  const helper = snapshot?.helpers?.find(item => item.request === target.request);
  return helper ? helperDetail(helper, input, width) : [];
}
/** The whole Activity › Now body: list + detail at SPLIT_MIN_WIDTH and above, picker + detail below it. */
export function renderActivityView(input: ActivityViewInput): ActivityView {
  const { snapshot, width } = input, height = Math.max(1, input.height), hits = new Map<number, ActivityTarget>();
  const row = (text: string) => PANEL_BG + PANEL_TEXT + pad(text, width) + RESET;
  if (!snapshot) return { lines: [row(" Activity unavailable: session snapshot disconnected.")], hits };
  const entries = activityEntries(snapshot, input.now), { entry, missing } = resolveSelection(entries, input.selected);
  const shown = entry ? { entry } : {};
  if (width < SPLIT_MIN_WIDTH) {
    const inner = Math.max(1, width - 2), detail = detailLines(input, entry, missing, inner, height - 2);
    const lines = [pickerRow(entries, entry, width, unavailableSources(snapshot)), row(""), ...detail.map(line => PANEL_BG + " " + PANEL_TEXT + pad(line, inner) + " " + RESET)];
    while (lines.length < height) lines.push(row(""));
    return { lines, hits, ...shown };
  }
  const inner = width - LIST_WIDTH - 3, detail = detailLines(input, entry, missing, inner, height), list = listPane(snapshot, entries, entry?.key);
  list.hits.forEach((target, index) => { if (target) hits.set(index, target); });
  const lines: string[] = [];
  for (let index = 0; index < Math.max(height, list.lines.length, detail.length); index++) {
    lines.push((list.lines[index] ?? LIST_BLANK) + PANEL_BG + "  " + PANEL_TEXT + pad(detail[index] ?? "", inner) + " " + RESET);
  }
  return { lines, hits, ...shown };
}

export function pagerLines(job: PanelJob): string[] {
  const d = job.detail;
  if (!d) return [];
  const all = numberedTail(d.tail, d.lines, job.state === "running"), gutter = String(all.at(-1)?.number ?? "").length;
  return all.map(line => (line.number === undefined ? "" : String(line.number).padStart(gutter) + " │ ") + line.text);
}
/** A bounded read-only overlay over the retained output; never reads or follows the job's output file. */
export async function openOutputPager(ui: ExtensionUIContext, title: string, lines: readonly string[], start = 0): Promise<void> {
  await ui.custom<void>((tui, _theme, _keys, done) => {
    let offset = start, max = 0;
    return { render: width => {
      const height = Math.max(1, Math.floor(tui.terminal.rows * 0.8) - 2);
      max = Math.max(0, lines.length - height); offset = Math.max(0, Math.min(max, offset));
      return [pad(plainLabel(title) + " · Esc close · ↑/↓ PgUp/PgDn", width), ...lines.slice(offset, offset + height).map(line => pad(line, width))];
    }, handleInput: data => {
      if (matchesKey(data, "escape") || data === "q") { done(); return; }
      const step = matchesKey(data, "up") ? -1 : matchesKey(data, "down") ? 1 : matchesKey(data, "pageUp") ? -10 : matchesKey(data, "pageDown") ? 10 : 0;
      offset = Math.max(0, Math.min(max, offset + step)); tui.requestRender();
    }, invalidate() {} };
  }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
}
