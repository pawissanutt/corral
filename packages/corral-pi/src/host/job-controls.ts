import type { Commands } from "../extensions/jobs/commands.ts";
import type { JobRuntime } from "../extensions/jobs/runtime.ts";

export type JobAction = "peek" | "background" | "stop";
export interface JobTarget { readonly job: string; readonly session: string; readonly generation: number }
export type JobControl = (target: JobTarget, action: JobAction) => Promise<Record<string, unknown>>;
interface Options {
  readonly jobs: JobRuntime; readonly commands: Commands;
  readonly generation: number;
  readonly session: () => string | undefined;
  readonly allowed: (tool: string) => boolean;
  readonly record: (entry: Record<string, unknown>) => void;
  readonly before?: (id: string, job: string) => Promise<string | undefined>;
  readonly after?: (id: string, job: string, failed: boolean) => Promise<void>;
}

/** Owner UI adapter, not a shell/PM backdoor. Revalidates the captured selection after dialogs/hooks. */
export function jobControl(options: Options): JobControl {
  let sequence = 0;
  const inflight = new Set<string>();
  return async (target, action) => {
    const tool = action === "stop" ? "job_kill" : action === "peek" ? "job_output" : "bash";
    const refusal = () => !options.session() || options.session() !== target.session || options.generation !== target.generation
      ? "stale or disconnected host" : !options.allowed(tool) ? `${tool} is not allowed`
      : !options.commands.owns(target.job) ? "unknown or foreign job" : undefined;
    const denied = refusal();
    if (denied) return { state: "denied", reason: denied };
    if (inflight.has(target.job)) return { state: "denied", reason: "control already in flight; inspect before retrying" };
    if (action === "peek") return options.jobs.peek(target.job, { tail_lines: 50 });
    const id = `job-control:${options.generation}:${++sequence}`;
    inflight.add(target.job);
    try {
      if (action === "stop") {
        const before = await options.before?.(id, target.job);
        const recheck = before ?? refusal();
        if (recheck) return { state: "denied", reason: recheck };
      }
      options.record({ id, ...target, action, phase: "started" });
      const result = action === "stop" ? await options.commands.kill(target.job) : options.commands.background(target.job);
      if (action === "stop") await options.after?.(id, target.job, false);
      options.record({ id, ...target, action, phase: "finished", result });
      return result;
    } catch (error) {
      // No automatic retry after hooks or signalling could have acted.
      const reason = error instanceof Error ? error.message : String(error);
      options.record({ id, ...target, action, phase: "outcome_unknown", reason });
      return { state: "outcome_unknown", reason, job: target.job };
    } finally { inflight.delete(target.job); }
  };
}
