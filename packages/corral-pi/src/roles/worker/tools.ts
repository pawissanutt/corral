// The worker's local resume note and engine completion tool.
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Errors } from "typebox/value";
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { sha256Hex } from "../../tools/corral.ts";
import { LIVE_CHILD_STATES, type FleetSnapshot } from "../../build/types.ts";
import { sessionRecord, type SessionRecordOptions } from "../../extensions/fleet/session-record.ts";

export const checkpointTool: CorralToolSpec = {
  name: "corral_checkpoint",
  label: "Save a resume note",
  description: "Save a short note in this session's transcript for whoever resumes the work.",
  op: "checkpoint",
  parameters: Type.Object({
    note: Type.String({ description: "Current state, remaining work and anything needed to resume safely" }),
  }, { additionalProperties: false }),
  payload: (params) => ({ note: params.note }),
};

export const doneTool: CorralToolSpec = {
  name: "corral_done",
  label: "Finish the run",
  description: "Report the run complete and move the task to Review. Collect or cancel any live review child "
    + "first. Include the advisory verdict and any deliberately unaddressed findings in the summary.",
  op: "done",
  parameters: Type.Object({
    summary: Type.String({ description: "The result, what was verified, and what was not verified" }),
  }, { additionalProperties: false }),
  payload: (params) => ({ summary: params.summary }),
};

/** Attach advisory fleet evidence without changing the done operation's authority or result. */
export function workerDoneTool(tool: ToolDefinition, host: SessionRecordOptions & {
  readonly recordDir: string;
  fleet(): FleetSnapshot;
  readonly usageArtifact?: () => string;
  attachArtifact(payload: { path: string; kind: string; title: string }): Promise<void>;
}): ToolDefinition {
  return { ...tool, async execute(id, params, signal, update, context) {
    // The underlying tool owns argument errors; invalid calls must not attach artifacts.
    if (Errors(doneTool.parameters, params).length) return tool.execute(id, params, signal, update, context);
    const fleet = host.fleet();
    const live = fleet.children.filter(child => child.kind === "review" && LIVE_CHILD_STATES.includes(child.state));
    if (live.length) {
      const text = `Review child ${live.map(child => child.id).join(", ")} is still running. Collect it first with agent_wait or cancel it with agent_cancel before corral_done.`;
      const value = { code: "review_running", reason: text };
      return { content: [{ type: "text", text }], details: value, structuredContent: value, isError: true };
    }
    let warning: string | undefined;
    if (fleet.children.length || fleet.mail.length) {
      try {
        const path = join(host.recordDir, `${sha256Hex(id)}.json`);
        mkdirSync(host.recordDir, { recursive: true, mode: 0o700 });
        if (!existsSync(path)) {
          writeFileSync(path, `${JSON.stringify(sessionRecord(fleet, { task: host.task, run: host.run }))}\n`, { flag: "wx", mode: 0o600 });
        }
        await host.attachArtifact({ path, kind: "log", title: "Session record" });
      } catch (error) {
        warning = `Session record attachment failed: ${error instanceof Error ? error.message : String(error)}`;
      }
    }
    if (host.usageArtifact) {
      try { await host.attachArtifact({ path: host.usageArtifact(), kind: "usage", title: "Model usage" }); }
      catch (error) {
        warning = [warning, `Usage attachment failed: ${error instanceof Error ? error.message : String(error)}`].filter(Boolean).join("\n");
      }
    }
    const result = await tool.execute(id, params, signal, update, context);
    return warning ? { ...result, content: [...result.content, { type: "text", text: warning }] } : result;
  } };
}
