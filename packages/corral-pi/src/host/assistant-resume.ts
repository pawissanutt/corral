// `/resume` in a supervised assistant: a Corral-owned picker over the workspace's assistant
// sessions. Selecting one writes a switch request and asks the supervisor to restart; the
// supervisor revalidates under the target's claim. Pi's native file switch stays refused.
import { existsSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { matchesKey, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { EventLog } from "../bridge/events.ts";
import type { HistoryRequest } from "../tools/history.ts";

/** Supervisor exit code: resume the session named in this generation's switch request. */
export const RESUME_ASSISTANT_EXIT = 77;
export const SWITCH_REQUEST_FILE = "resume-request.json";
export const SWITCH_NOTICE_FILE = "resume-notice.txt";

export type Availability = "current" | "live" | "ended" | "stopped";
export interface ResumeRow {
  readonly id: string;
  readonly title: string;
  readonly lastActivity: string;
  readonly availability: Availability;
}

const text = (value: unknown): string => typeof value === "string" ? value.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ") : "";

/** Rows from `corral assistant list --json`, newest activity first; malformed entries are dropped. */
export function resumeRows(value: unknown, current: string): ResumeRow[] {
  if (!Array.isArray(value)) throw new Error("assistant list response is not a list");
  const rows: ResumeRow[] = [];
  for (const entry of value) {
    if (entry === null || typeof entry !== "object") continue;
    const row = entry as Record<string, unknown>;
    const id = text(row.session);
    if (!/^A-\d+$/.test(id)) continue;
    const focus = text(row.focus);
    const title = text(row.summary) || (focus && focus !== "workspace" ? `focus ${focus}` : "no summary recorded");
    const availability: Availability = id === current ? "current" : row.ended ? "ended" : row.live === true ? "live" : "stopped";
    rows.push({ id, title, lastActivity: text(row.last_activity) || text(row.created), availability });
  }
  return rows.sort((a, b) => b.lastActivity.localeCompare(a.lastActivity));
}

const LABEL: Record<Availability, string> = {
  current: "current · this pane", live: "live · host running elsewhere", ended: "ended · not resumable", stopped: "stopped · resumable",
};
const activity = (row: ResumeRow): string => row.lastActivity.slice(0, 16).replace("T", " ") || "unknown";
export function rowLabel(row: ResumeRow): string {
  return `${row.id} · ${row.title} · ${activity(row)} · ${LABEL[row.availability]}`;
}

/** Narrowest summary kept on the row's line; below it activity and availability get their own line. */
const MIN_TITLE = 8;
/**
 * One row in terminal cells: the summary is truncated so activity and availability always fit.
 * When `width` is too narrow for that, they move to a second, indented line.
 */
export function rowLines(row: ResumeRow, width: number, marker: string): string[] {
  const head = `${marker}${row.id} · `, tail = ` · ${activity(row)} · ${LABEL[row.availability]}`;
  const room = width - visibleWidth(head) - visibleWidth(tail);
  if (room >= MIN_TITLE) return [`${head}${truncateToWidth(row.title, room, "…")}${tail}`];
  const indent = " ".repeat(visibleWidth(marker) + 2);
  const state = `${indent}${LABEL[row.availability]}`, when = ` · ${activity(row)}`;
  const details = visibleWidth(state) + visibleWidth(when) <= width ? [`${state}${when}`] : [state, `${indent}${activity(row)}`];
  return [truncateToWidth(`${head}${row.title}`, width, "…"), ...details.map(line => truncateToWidth(line, width, "…"))];
}
export function filterRows(rows: readonly ResumeRow[], filter: string): ResumeRow[] {
  const needle = filter.trim().toLowerCase();
  return needle ? rows.filter(row => rowLabel(row).toLowerCase().includes(needle)) : [...rows];
}

/** Why a row cannot be resumed from here, or undefined when it is eligible. */
export function ineligible(row: ResumeRow | undefined, id: string): string | undefined {
  if (!row) return `corral: ${id} is no longer an assistant session in this workspace`;
  switch (row.availability) {
    case "current": return `corral: ${id} is the current session; continue here`;
    case "live": return `corral: ${id}'s host is running in another pane; continue there, or end it with \`corral assistant end ${id}\``;
    case "ended": return `corral: ${id} was ended with \`corral assistant end\` and stays ended; /new opens a new assistant`;
    case "stopped": return undefined;
  }
}

export type Chooser = (ui: ExtensionUIContext, rows: readonly ResumeRow[]) => Promise<ResumeRow | undefined>;

interface ResumeOptions {
  readonly assistant: string;
  readonly generation: number;
  readonly runtimeDir: string;
  readonly events: EventLog;
  readonly read: (request: HistoryRequest) => Promise<unknown>;
  /** True when no turn, bash command or handoff is in flight. */
  readonly idle: () => boolean;
  /** Marks the host closing and exits with RESUME_ASSISTANT_EXIT. */
  readonly request: () => void;
  readonly choose?: Chooser;
}

export class AssistantResume {
  private busy = false;
  constructor(private readonly options: ResumeOptions) {}

  extend(pi: ExtensionAPI): void {
    pi.registerCommand("resume", {
      description: "Resume another corral assistant session in this pane",
      handler: async (_args, ctx) => {
        if (ctx.mode !== "tui" || !ctx.hasUI) return;
        await this.open(ctx.ui, () => ctx.isIdle());
      },
    });
    pi.on("session_start", (event, ctx) => {
      if (event.reason !== "startup") return;
      const path = join(this.options.runtimeDir, SWITCH_NOTICE_FILE);
      if (!existsSync(path)) return;
      const notice = readFileSync(path, "utf8").slice(0, 2048);
      unlinkSync(path);
      this.options.events.emit("assistant_resume_notice", { text: notice.slice(0, 256) });
      if (ctx.hasUI) ctx.ui.notify(notice, "warning");
    });
  }

  async open(ui: ExtensionUIContext, piIdle: () => boolean): Promise<void> {
    const o = this.options;
    if (this.busy) return;
    if (!piIdle() || !o.idle()) { ui.notify("corral: wait for the current work to finish before /resume", "warning"); return; }
    this.busy = true;
    try {
      let rows: ResumeRow[];
      try { rows = resumeRows(await o.read({ kind: "assistants" }), o.assistant); }
      catch (error) {
        ui.notify(`corral: assistant sessions unavailable: ${error instanceof Error ? error.message : String(error)}`, "warning");
        return;
      }
      if (!rows.some(row => row.availability !== "current")) {
        ui.notify("corral: no other assistant sessions in this workspace; /new opens one", "info");
        return;
      }
      const chosen = await (o.choose ?? choose)(ui, rows);
      if (!chosen) { o.events.emit("assistant_resume_cancelled", {}); return; }
      // Revalidate at selection time; the supervisor checks again under the target's claim.
      let fresh: ResumeRow | undefined;
      try { fresh = resumeRows(await o.read({ kind: "assistants" }), o.assistant).find(row => row.id === chosen.id); }
      catch (error) {
        ui.notify(`corral: could not recheck ${chosen.id}: ${error instanceof Error ? error.message : String(error)}`, "warning");
        return;
      }
      const refusal = ineligible(fresh, chosen.id);
      if (refusal) {
        o.events.emit("assistant_resume_refused", { target: chosen.id, availability: fresh?.availability ?? "missing" });
        ui.notify(refusal, "warning");
        return;
      }
      if (!piIdle() || !o.idle()) { ui.notify("corral: wait for the current work to finish before /resume", "warning"); return; }
      const path = join(o.runtimeDir, SWITCH_REQUEST_FILE);
      const temporary = `${path}.tmp`;
      writeFileSync(temporary, JSON.stringify({ version: 1, assistant: o.assistant, generation: o.generation, target: chosen.id }),
        { mode: 0o600 });
      renameSync(temporary, path);
      o.events.emit("assistant_resume_requested", { target: chosen.id });
      ui.notify(`corral: suspending ${o.assistant} and resuming ${chosen.id}…`, "info");
      o.request();
    } finally { this.busy = false; }
  }
}

/** A searchable list: type to filter, ↑↓ to move, Enter to select, Esc to cancel. */
export const choose: Chooser = (ui, rows) => ui.custom<ResumeRow | undefined>((tui, theme, _keys, done) => {
  let filter = "", index = 0;
  const visible = () => filterRows(rows, filter);
  return {
    render: (width: number) => {
      const shown = visible(), budget = Math.max(3, Math.min(12, tui.terminal.rows - 6));
      index = Math.min(index, Math.max(0, shown.length - 1));
      const cut = (line: string) => truncateToWidth(line, Math.max(1, width), "…");
      const style = (row: ResumeRow, selected: boolean) => (line: string) =>
        selected ? theme.fg("accent", line) : row.availability === "stopped" ? line : theme.fg("dim", line);
      const lines = (i: number) => rowLines(shown[i]!, Math.max(1, width), i === index ? "› " : "  ");
      // A window of whole rows around the selection, filling `budget` terminal lines.
      let start = index, end = index + 1, used = shown.length ? lines(index).length : 0;
      while (true) {
        const below = end < shown.length ? lines(end).length : 0, above = start > 0 ? lines(start - 1).length : 0;
        if (below && used + below <= budget && (end - index <= index - start || !above)) { used += below; end++; }
        else if (above && used + above <= budget) { used += above; start--; }
        else if (below && used + below <= budget) { used += below; end++; }
        else break;
      }
      return [
        cut(theme.bold(`Resume an assistant session (${shown.length}/${rows.length}) · current conversation and draft stay until you select`)),
        cut(`Search: ${filter}▏`),
        ...(shown.length ? shown.slice(start, end).flatMap((row, i) => lines(start + i).map(style(row, start + i === index)))
          : [theme.fg("dim", cut("  No matching assistant sessions"))]),
        cut(theme.fg("dim", "type to search · ↑↓ move · Enter resume · Esc cancel")),
      ];
    },
    handleInput: (data: string) => {
      const shown = visible();
      if (matchesKey(data, "escape")) done(undefined);
      else if (matchesKey(data, "enter")) { if (shown[index]) done(shown[index]); }
      else if (matchesKey(data, "up")) index = shown.length ? (index + shown.length - 1) % shown.length : 0;
      else if (matchesKey(data, "down")) index = shown.length ? (index + 1) % shown.length : 0;
      else if (matchesKey(data, "backspace")) { filter = filter.slice(0, -1); index = 0; }
      else if (data.length === 1 && data >= " " && data !== "\u007f") { filter += data; index = 0; }
      tui.requestRender();
    },
    invalidate: () => undefined,
  };
});
