// `/resume` in a supervised assistant: a Corral-owned picker over the workspace's assistant
// sessions. Selecting one writes a switch request and asks the supervisor to restart; the
// supervisor revalidates under the target's claim. Pi's native file switch stays refused.
import { existsSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
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
  /** The picker; a pick is revalidated here before anything is requested. */
  readonly choose: Chooser;
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
      const chosen = await o.choose(ui, rows);
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
