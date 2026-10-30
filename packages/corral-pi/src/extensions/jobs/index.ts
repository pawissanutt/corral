import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { JobError, type JobRuntime, OUTPUT_READ_MAX } from "./runtime.ts";

export const JOB_TOOL_NAMES = ["job_start", "job_output", "job_kill", "corral_job_monitor"] as const;
export const JOB_MONITOR_TOOL = "corral_job_monitor";

function text(value: unknown) {
  return { content: [{ type: "text" as const, text: JSON.stringify(value) }], details: undefined };
}

async function guarded(run: () => Promise<unknown> | unknown) {
  try {
    return text(await run());
  } catch (error) {
    if (error instanceof JobError) return text({ error: error.message });
    throw error;
  }
}

export function jobTools(jobs: JobRuntime): readonly ToolDefinition[] {
  return [
    {
      name: "job_start",
      label: "Start a background job",
      description: "Run a shell command in the background in the worktree and return its job ID at once. "
        + "Use it for builds, tests and servers that take a while, then keep working. Output is captured "
        + "and bounded; the job is killed at its runtime limit, when you call job_kill, or when this run ends.",
      parameters: Type.Object({ command: Type.String({ minLength: 1, maxLength: 4096 }) }),
      async execute(id, params: { command: string }) {
        return guarded(() => jobs.start(params.command, id));
      },
    },
    {
      name: "job_output",
      label: "Read background job output",
      description: "Read a job's state, exit code and captured output. Without offset it returns the latest "
        + `output; with offset it pages forward from there (next_offset). At most ${OUTPUT_READ_MAX} bytes per call.`,
      parameters: Type.Object({
        job: Type.String(),
        offset: Type.Optional(Type.Integer({ minimum: 0 })),
        max_bytes: Type.Optional(Type.Integer({ minimum: 1, maximum: OUTPUT_READ_MAX })),
      }),
      async execute(_id, params: { job: string; offset?: number; max_bytes?: number }) {
        return guarded(() => jobs.output(params.job, params.offset, params.max_bytes));
      },
    },
    {
      name: "job_kill",
      label: "Stop a background job",
      description: "Stop a job's whole process group (SIGTERM, then SIGKILL after a short grace) and return its "
        + "final state.",
      parameters: Type.Object({ job: Type.String() }),
      async execute(_id, params: { job: string }) {
        return guarded(() => jobs.kill(params.job));
      },
    },
    {
      name: JOB_MONITOR_TOOL,
      label: "Wake when a job exits or prints a line",
      description: "Ask corral to wake you once when a job exits (on: exit) or prints a line matching a "
        + "JavaScript regular expression (on: line). The wake arrives as a message even if it already happened; "
        + "end your turn after this call instead of polling job_output. Without a match before timeout_secs, "
        + "the wake reports that the deadline expired.",
      parameters: Type.Object({
        job: Type.String(),
        on: Type.Union([Type.Literal("exit"), Type.Literal("line")]),
        pattern: Type.Optional(Type.String({ minLength: 1, maxLength: 512 })),
        timeout_secs: Type.Optional(Type.Integer({ minimum: 1, maximum: 86400 })),
      }),
      async execute(id, params: { job: string; on: "exit" | "line"; pattern?: string; timeout_secs?: number }) {
        return guarded(async () => {
          const receipt = await jobs.monitor(params.job, params.on, params.pattern, params.timeout_secs ?? 3600, id);
          return { state: receipt.state, result: receipt.result };
        });
      },
    },
  ];
}
