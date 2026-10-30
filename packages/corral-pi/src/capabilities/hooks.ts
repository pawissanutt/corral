// Configured worker argv hooks (T-572): pre-tool, post-tool and turn-end programs run without a
// shell in the worktree, outside the in-process network guard. Before each launch the host
// appends a durable `corral_hook` entry to the bound session, then `started` and `completed`;
// recovery reads them so a lost hook makes its call outcome_unknown. Nothing is replayed.
import { spawn } from "node:child_process";

import { z } from "zod";

export const HOOK_ENTRY = "corral_hook";
const OUTPUT_MAX_BYTES = 8 * 1024;
const STDIN_MAX_BYTES = 256 * 1024;

const Hook = z.object({
  name: z.string().regex(/^[A-Za-z0-9._-]{1,64}$/),
  argv: z.array(z.string().max(4096).refine((arg) => !arg.includes("\0"))).min(1).max(64)
    .refine((argv) => (argv[0] ?? "").length > 0),
  tools: z.array(z.string().regex(/^[a-z][a-z0-9_]{0,63}$/).refine((tool) => !tool.startsWith("corral_")))
    .max(64).optional(),
}).strict().readonly();
const Phase = z.array(Hook).max(16).readonly();

export const HookSettingsSchema = z.object({
  timeout_ms: z.number().int().min(1).max(600_000),
  pre_tool: Phase,
  post_tool: Phase,
  turn_end: Phase.refine((hooks) => hooks.every((hook) => hook.tools === undefined)),
}).strict().readonly();

export type HookSettings = z.infer<typeof HookSettingsSchema>;
type HookCommand = z.infer<typeof Hook>;
export type HookPhase = "pre_tool" | "post_tool" | "turn_end";

export interface HookRecord {
  readonly invocation: string;
  readonly phase: HookPhase;
  readonly hook: string;
  readonly state: "prepared" | "started" | "completed";
  readonly tool?: string;
  readonly tool_call_id?: string;
  readonly turn?: number;
  readonly pid?: number;
  readonly exit_code?: number | null;
  readonly signal?: string | null;
  readonly timed_out?: boolean;
  readonly error?: string;
}

export interface HookHost {
  readonly cwd: string;
  readonly generation: number;
  /** Appends one durable record; it must be written before the hook is launched. */
  record(entry: HookRecord): void;
}

export interface ToolScope {
  readonly tool: string;
  readonly toolCallId: string;
  readonly input: Record<string, unknown>;
  readonly turn?: number;
}

interface Outcome {
  readonly ok: boolean;
  readonly summary: string;
}

function tail(text: string): string {
  const bytes = Buffer.from(text);
  return bytes.length <= OUTPUT_MAX_BYTES ? text : `…${bytes.subarray(bytes.length - OUTPUT_MAX_BYTES).toString()}`;
}

export class Hooks {
  private seq = 0;

  constructor(private readonly settings: HookSettings, private readonly host: HookHost) {}

  private matching(hooks: readonly HookCommand[], tool: string): HookCommand[] {
    if (tool.startsWith("corral_")) return [];
    return hooks.filter((hook) => hook.tools === undefined || hook.tools.length === 0 || hook.tools.includes(tool));
  }

  wraps(tool: string): boolean {
    return this.matching([...this.settings.pre_tool, ...this.settings.post_tool], tool).length > 0;
  }

  /** Runs the pre-tool hooks in order; a failing hook refuses the call with its reason. */
  async before(call: ToolScope): Promise<string | undefined> {
    for (const hook of this.matching(this.settings.pre_tool, call.tool)) {
      const outcome = await this.run("pre_tool", hook, call, { input: call.input });
      if (!outcome.ok) return `pre_tool hook ${hook.name} refused ${call.tool}: ${outcome.summary}`;
    }
    return undefined;
  }

  /** Runs every post-tool hook; failures become notes appended to the tool result. */
  async after(call: ToolScope, isError: boolean): Promise<string[]> {
    const notes: string[] = [];
    for (const hook of this.matching(this.settings.post_tool, call.tool)) {
      const outcome = await this.run("post_tool", hook, call, { input: call.input, is_error: isError });
      if (!outcome.ok) notes.push(`[post_tool hook ${hook.name}: ${outcome.summary}]`);
    }
    return notes;
  }

  async turnEnd(turn: number | undefined): Promise<void> {
    for (const hook of this.settings.turn_end) {
      await this.run("turn_end", hook, turn === undefined ? {} : { turn }, {});
    }
  }

  private run(
    phase: HookPhase, hook: HookCommand, scope: Partial<ToolScope>, extra: Record<string, unknown>,
  ): Promise<Outcome> {
    const invocation = `hk-${this.host.generation}-${++this.seq}`;
    const base = {
      invocation, phase, hook: hook.name,
      ...(scope.tool !== undefined ? { tool: scope.tool } : {}),
      ...(scope.toolCallId !== undefined ? { tool_call_id: scope.toolCallId } : {}),
      ...(scope.turn !== undefined ? { turn: scope.turn } : {}),
    };
    let stdin = JSON.stringify({ ...base, generation: this.host.generation, ...extra });
    if (Buffer.byteLength(stdin) > STDIN_MAX_BYTES) {
      stdin = JSON.stringify({ ...base, generation: this.host.generation, input: { omitted: "too_large" } });
    }
    this.host.record({ ...base, state: "prepared" });
    return new Promise((resolve) => {
      let settled = false;
      let timedOut = false;
      let output = "";
      const finish = (code: number | null, signal: string | null, error?: string) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        this.host.record({
          ...base, state: "completed", exit_code: code, signal,
          ...(timedOut ? { timed_out: true } : {}), ...(error ? { error: error.slice(0, 512) } : {}),
        });
        const reason = error ?? (timedOut ? `timed out after ${this.settings.timeout_ms} ms`
          : signal ? `killed by ${signal}` : `exited ${code}`);
        resolve({ ok: !error && !timedOut && code === 0, summary: `${reason}${output ? `: ${tail(output).trim()}` : ""}` });
      };
      const child = spawn(hook.argv[0] ?? "", hook.argv.slice(1), {
        cwd: this.host.cwd, detached: true, stdio: ["pipe", "pipe", "pipe"],
        env: {
          ...process.env, CORRAL_HOOK_PHASE: phase, CORRAL_HOOK_NAME: hook.name, CORRAL_HOOK_INVOCATION: invocation,
          ...(scope.tool !== undefined ? { CORRAL_HOOK_TOOL: scope.tool } : {}),
        },
      });
      const timer = setTimeout(() => {
        timedOut = true;
        try {
          if (child.pid !== undefined) process.kill(-child.pid, "SIGKILL");
        } catch {
          child.kill("SIGKILL");
        }
      }, this.settings.timeout_ms);
      const collect = (chunk: Buffer) => {
        output = tail(output + chunk.toString());
      };
      child.stdout?.on("data", collect);
      child.stderr?.on("data", collect);
      child.on("spawn", () => this.host.record({ ...base, state: "started", ...(child.pid ? { pid: child.pid } : {}) }));
      child.on("error", (error) => finish(null, null, error.message));
      child.on("close", (code, signal) => finish(code, signal));
      child.stdin?.on("error", () => undefined);
      child.stdin?.end(stdin);
    });
  }
}
