// Background job detail: the card under the agents table when a job row is selected, and the bounded
// read-only output pager. Values are this host's job runtime observations, never task settlement.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { ScrollView, matchesKey, stripTerminalSequences, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { chipRows, jobStatus, type Chip } from "./agents-model.ts";
import {
  box, pad, plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_RULE, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING,
  type PanelJob,
} from "./panels-view.ts";

/** Output captured when follow was switched off; the detail keeps showing it until follow resumes. */
export interface JobFreeze { readonly job: string; readonly tail: string; readonly lines: number | undefined; readonly running: boolean }
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

/**
 * The job card: command, state chips, an optional legend, then the numbered retained tail sized to `height`
 * rows. A stale host or a lost job shows why output is withheld instead of retained text.
 */
export function jobDetailLines(job: PanelJob, input: { readonly bound: boolean; readonly now: number; readonly legend: boolean; readonly freeze: JobFreeze | undefined },
  width: number, height: number): string[] {
  const d = job.detail, status = jobStatus(job, input.now);
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
  if (!input.bound) chips.push({ text: "stale", tone: "warning" });
  const cardBody = [head, ...chipRows(chips, width - 4)];
  const rows = box("", cardBody, width, cardBody.length + 2);
  if (input.legend) {
    const body = ["Source: this host's job runtime only; a job missing here is not completed.",
      "Exit status is a local observation: it does not imply gate PASS or task settlement.",
      ...(d ? [`Process group ${d.groupClear ? "observed clear" : "not confirmed clear"}.`, `Monitoring · ${d.monitorAttempts} attempts (not active-wait count).`] : []),
      ...(d?.watches ?? []).flatMap(w => [`Watch ${plainLabel(w.wait)} /${plainLabel(w.pattern)}/ · deadline ${plainLabel(w.deadline)}${w.reportQueued ? " · report queued, settlement unknown" : ""}`,
        ...(w.matched !== undefined ? ["Matched: " + plainLabel(w.matched)] : [])]),
      "Peek is nonblocking. Background releases observation, not the process. Stop confirms before signalling the owned group. There is no restart.",
      "A line match is not service health. Stale, foreign or denied controls never signal a process."]
      .flatMap(line => wrapTextWithAnsi(line, Math.max(1, width - 4))).map(line => PANEL_MUTED + line);
    rows.push(...box("Legend", body, width, body.length + 2, true));
  }
  if (job.state === "lost" || !d) return [...rows, PANEL_MUTED + "Detailed output unavailable for this job source."];
  if (!input.bound) return [...rows, PANEL_WARNING + "Output unavailable: stale/disconnected host"];
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
  rows.push(PANEL_MUTED + (frozen ? `‖ follow paused · ${count} at pause` : source.running ? `▼ following tail · ${count} · ${stored}` : `■ exited · ${count} · ${stored}`));
  return rows;
}

/** Every retained output line with its host line number, for the pager. */
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
      return [pad(plainLabel(title), width), ...lines.slice(offset, offset + height).map(line => pad(line, width))];
    }, handleInput: data => {
      if (matchesKey(data, "escape") || data === "q") { done(); return; }
      const step = matchesKey(data, "up") ? -1 : matchesKey(data, "down") ? 1 : matchesKey(data, "pageUp") ? -10 : matchesKey(data, "pageDown") ? 10 : 0;
      offset = Math.max(0, Math.min(max, offset + step)); tui.requestRender();
    }, invalidate() {} };
  }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
}
