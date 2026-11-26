import { randomUUID } from "node:crypto";
import type { Commands } from "../extensions/jobs/commands.ts";
import type { JobRuntime } from "../extensions/jobs/runtime.ts";

export type JobAction = "peek" | "background" | "stop";
export interface JobTarget { readonly job: string; readonly session: string }
export type JobControl = (target: JobTarget, action: JobAction) => Promise<Record<string, unknown>>;
interface Options {
  readonly jobs: JobRuntime; readonly commands: Commands;
  readonly session: () => string | undefined;
  readonly allowed: (tool: string) => boolean;
  readonly record: (entry: Record<string, unknown>) => void;
  readonly before?: (id: string, job: string) => Promise<string | undefined>;
  readonly after?: (id: string, job: string, failed: boolean) => Promise<void>;
}

/** Owner UI adapter, not a shell/PM backdoor. Revalidates the captured selection after dialogs/hooks. */
export function jobControl(options: Options): JobControl {
  const inflight = new Set<string>();
  return async ({ job, session }, action) => {
    const tool = action === "stop" ? "job_kill" : action === "peek" ? "job_output" : "bash";
    const refusal = () => !session || options.session() !== session
      ? "stale or disconnected host" : !options.allowed(tool) ? `${tool} is not allowed`
      : !options.commands.owns(job) ? "unknown or foreign job" : undefined;
    const denied = refusal();
    if (denied) return { state: "denied", reason: denied };
    if (inflight.has(job)) return { state: "denied", reason: "control already in flight; inspect before retrying" };
    if (action === "peek") return options.jobs.peek(job, { tail_lines: 50 });
    const id = `job-control:${randomUUID()}`;
    const entry = { id, job, session, action };
    inflight.add(job);
    try {
      if (action === "stop") {
        const before = await options.before?.(id, job);
        const recheck = before ?? refusal();
        if (recheck) return { state: "denied", reason: recheck };
      }
      options.record({ ...entry, phase: "started" });
      const result = action === "stop" ? await options.commands.kill(job) : options.commands.background(job);
      const confirmed = result.stop_outcome === "stopped" || result.stop_outcome === "already_finished";
      if (action === "stop") await options.after?.(id, job, !confirmed);
      const ownerResult = action !== "stop" ? result : confirmed ? { ...result, state: result.stop_outcome }
        : { ...result, state: "outcome_unknown", reason: "Process-group exit could not be confirmed" };
      options.record({ ...entry, phase: "finished", result: ownerResult });
      return ownerResult;
    } catch (error) {
      // No automatic retry after hooks or signalling could have acted.
      const reason = error instanceof Error ? error.message : String(error);
      options.record({ ...entry, phase: "outcome_unknown", reason });
      return { state: "outcome_unknown", reason, job };
    } finally { inflight.delete(job); }
  };
}
