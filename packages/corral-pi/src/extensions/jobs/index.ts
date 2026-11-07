import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { callKey, nestedChild } from "../codemode/child.ts";
import { type Commands } from "./commands.ts";
import { JobError, type JobRuntime, OUTPUT_READ_MAX } from "./runtime.ts";
import type { OutputOptions } from "./output.ts";
import { renderBashResult, renderJobResult } from "./presentation.ts";

export const JOB_TOOL_NAMES = ["job_start", "job_output", "job_wait", "job_kill", "corral_job_monitor"] as const;
export const JOB_MONITOR_TOOL = "corral_job_monitor";

function text(value: unknown, nested: boolean) {
  // Legacy job tools resolve to JSON strings in Code Mode. Change presentation, not that API.
  return { content: [{ type: "text" as const, text: nested ? JSON.stringify(value) : renderJobResult(value) }], details: value };
}
async function guarded(name: string, id: string, run: () => Promise<unknown> | unknown, commandResult = false, bashStdout = false) {
  const nested = nestedChild(name, id) !== undefined;
  try {
    const value = await run();
    // Ordinary successful Bash keeps stdout; failures and partial output also expose status.
    const result = bashStdout && value !== null && typeof value === "object" && "state" in value && value.state === "exited" && "text" in value && typeof value.text === "string"
      ? { content: [{ type: "text" as const, text: renderBashResult({ ...value, text: value.text }) }], details: value, structuredContent: {
        output: value.text, state: "exited", exit_code: "exit_code" in value && typeof value.exit_code === "number" ? value.exit_code : null,
        signal: "signal" in value && typeof value.signal === "string" ? value.signal : null,
        truncated: "truncated" in value && value.truncated === true,
      } }
      : text(value, nested);
    if (commandResult && value !== null && typeof value === "object" && "state" in value && value.state === "exited" &&
      ("exit_code" in value && value.exit_code !== 0 || "signal" in value && value.signal !== null)) return { ...result, isError: true };
    return result;
  } catch (error) {
    if (error instanceof JobError) return { ...text({ error: error.message }, nested), isError: true };
    throw error;
  }
}
const waitBudget = Type.Optional(Type.Integer({ minimum: 0, maximum: 600000 }));

export function jobTools(jobs: JobRuntime, monitor = true, commands?: Commands): readonly ToolDefinition[] {
  const tools: ToolDefinition[] = [
    ...(commands ? [{
      name: "bash", label: "Run a managed command",
      description: "Execute bash in the worktree. wait_ms releases observation after 10000 ms by default, returning a job handle without restarting or killing the command. "
        + "timeout is a separate hard deadline in SECONDS from launch (bounded by the configured runtime cap). Completion is announced automatically. "
        + "Use job_output for a nonblocking log peek; job_wait to await one job or the next result. In Code Mode, the default awaits actual completion; explicit wait_ms:0 backgrounds immediately.",
      parameters: Type.Object({ command: Type.String({ minLength: 1, maxLength: 4096 }),
        timeout: Type.Optional(Type.Number({ minimum: 1 })), wait_ms: waitBudget }),
      async execute(id: string, params: { command: string; timeout?: number; wait_ms?: number }, signal?: AbortSignal) {
        return guarded("bash", id, () => commands.run(params.command, callKey("bash", id), {
          ...params, ...(signal ? { signal } : {}), nested: nestedChild("bash", id) !== undefined }), true, true);
      },
    } satisfies ToolDefinition] : []),
    {
      name: "job_start", label: "Start a background command",
      description: "Compatibility adapter for bash with wait_ms:0: launch one managed command and return its job ID immediately. "
        + "Turn endings do not stop it. The hard runtime cap, job_kill, or owning host teardown stops its process group. Completion is announced automatically.",
      parameters: Type.Object({ command: Type.String({ minLength: 1, maxLength: 4096 }) }),
      async execute(id, params: { command: string }, signal) {
        return guarded("job_start", id, () => commands ? commands.run(params.command, callKey("job_start", id), { wait_ms: 0, ...(signal ? { signal } : {}) })
          : jobs.start(params.command, callKey("job_start", id)));
      },
    },
    {
      name: "job_output", label: "Peek command output",
      description: "Nonblocking peek: return current logs and state now, even while the command is running. "
        + "Default is the latest 50 lines within 16 KiB; tail_lines selects recent lines. cursor (or legacy offset) pages independently for each reader. "
        + `At most ${OUTPUT_READ_MAX} bytes; retention gaps are explicit. Peeking never consumes completion.`,
      parameters: Type.Object({ job: Type.String(), offset: Type.Optional(Type.Integer({ minimum: 0 })),
        cursor: Type.Optional(Type.String()), tail_lines: Type.Optional(Type.Integer({ minimum: 1, maximum: 1000 })),
        max_bytes: Type.Optional(Type.Integer({ minimum: 1, maximum: OUTPUT_READ_MAX })) }),
      async execute(id, params: { job: string } & OutputOptions) {
        return guarded("job_output", id, () => commands ? commands.output(params.job, params) : jobs.peek(params.job, params));
      },
    },
    ...(commands ? [{
      name: "job_wait", label: "Await command completion",
      description: "Await one owned job, or omit job for the next unconsumed completion. Return immediately if none exist. "
        + "wait_ms (default 60000) limits observation only: expiry or cancellation never kills work. Completed output remains readable; no polling is needed.",
      parameters: Type.Object({ job: Type.Optional(Type.String()), wait_ms: waitBudget }),
      async execute(id: string, params: { job?: string; wait_ms?: number }, signal?: AbortSignal) {
        return guarded("job_wait", id, () => commands.wait(params.job, params.wait_ms, signal), true);
      },
    } satisfies ToolDefinition] : []),
    {
      name: "job_kill", label: "Stop a command",
      description: "Stop this host's owned process group (SIGTERM then SIGKILL after a short grace), retaining output. Never signals a stale or reused PID.",
      parameters: Type.Object({ job: Type.String() }),
      async execute(id, params: { job: string }) { return guarded("job_kill", id, () => commands ? commands.kill(params.job) : jobs.kill(params.job)); },
    },
    {
      name: JOB_MONITOR_TOOL, label: "Wake on command exit or matching line",
      description: "Arm one exit or JavaScript-regex line wake for an owned job. End your turn rather than polling. "
        + "A timeout_secs deadline wakes without killing work. Worker waits remain receipted by corral; PM/assistant watches are session-owned and report interruption after host loss.",
      parameters: Type.Object({ job: Type.String(), on: Type.Union([Type.Literal("exit"), Type.Literal("line")]),
        pattern: Type.Optional(Type.String({ minLength: 1, maxLength: 512 })),
        timeout_secs: Type.Optional(Type.Integer({ minimum: 1, maximum: 86400 })) }),
      async execute(id, params: { job: string; on: "exit" | "line"; pattern?: string; timeout_secs?: number }) {
        return guarded(JOB_MONITOR_TOOL, id, async () => {
          const runtime = commands ?? jobs;
          const receipt = await runtime.monitor(params.job, params.on, params.pattern, params.timeout_secs ?? 3600, callKey(JOB_MONITOR_TOOL, id));
          return { state: receipt.state, result: receipt.result };
        });
      },
    },
  ];
  return monitor ? tools : tools.filter(tool => tool.name !== JOB_MONITOR_TOOL);
}
