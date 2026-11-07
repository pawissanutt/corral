// Foreground shell is core host control, not the optional background_shell tool surface.
// Reuse its verified launcher/group sweep: a returned shell Promise alone proves nothing
// about descendants. An unverified exit leaves the separate durable activity unsettled.
import type { BashOperations } from "@earendil-works/pi-coding-agent";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { constants, tmpdir } from "node:os";
import { join } from "node:path";
import { JobRuntime } from "../../extensions/jobs/runtime.ts";
import type { RegisterCapability } from "../../extensions/registry.ts";
import type { PlanningState } from "../../host/planning-policy.ts";

export function requireVerifiedPlanningDrain(supported: boolean): void {
  if (!supported) throw new Error("assistant_planning: unsupported verified shell drain on this host; Planning activation/drain not started");
}

/** Unsupported hosts keep ordinary SDK behavior, but can never activate/acknowledge Planning. */
export function assistantShellBackend(supported: boolean, state: PlanningState): "managed" | "sdk" {
  if (supported) return "managed";
  if (state.phase !== "ordinary" || state.expected || state.boundary?.uncertain
      || state.boundary?.activities.some(activity => activity.uncertain)) {
    throw new Error("assistant_planning: unsupported verified shell drain in a protected/uncertain session; no effect started");
  }
  return "sdk";
}

export class AssistantShell {
  // Generation-pinned: a host that ran unverified ordinary SDK work must not later claim
  // a verified drain merely because platform facilities appeared while it was running.
  readonly verifiedDrain = process.platform === "linux" && existsSync("/proc/self/stat");
  private sequence = 0;
  private root: string | undefined;
  private closed = false;
  private stopping: Promise<void> | undefined;
  private readonly runtimes = new Set<JobRuntime>();
  private readonly pending = new Set<Promise<{ exitCode: number | null }>>();
  readonly operations: BashOperations = { exec: (...args) => {
    const operation = this.execute(...args);
    this.pending.add(operation);
    void operation.then(() => this.pending.delete(operation), () => this.pending.delete(operation));
    return operation;
  } };

  constructor(private readonly options: {
    readonly generation: number; readonly runtimeDir?: string;
    readonly registerCapability: RegisterCapability;
    admit(id: string): Promise<void>;
    settle(id: string): Promise<void>;
  }) {}

  private async execute(command: string, cwd: string, options: Parameters<BashOperations["exec"]>[2]): Promise<{ exitCode: number | null }> {
    if (this.closed || this.stopping) throw new Error("assistant shell is stopping; no effect started");
    if (options.signal?.aborted) throw new Error("aborted");
    // The reused runtime's identity/group proof is Linux /proc-based. Never silently fall
    // back to an unowned SDK subprocess on an unsupported platform.
    if (!this.verifiedDrain || !existsSync("/proc/self/stat")) throw new Error("assistant shell requires verified process-group ownership; no effect started");
    this.root ??= this.options.runtimeDir ? join(this.options.runtimeDir, "foreground-shell") : mkdtempSync(join(tmpdir(), "corral-assistant-shell-"));
    const invocation = `s${++this.sequence}`, directory = join(this.root, invocation);
    const id = `j${this.options.generation}-1`, resource = `${invocation}:${id}`;
    let offset = 0, clean = false, acknowledged = false, aborted: Promise<unknown> | undefined;
    // One runtime per invocation preserves each SDK call's cwd/environment and isolates IDs
    // and output cursors from optional background jobs (without enabling that capability).
    const runtime = new JobRuntime({ cwd, runtimeDir: directory, generation: this.options.generation,
      settings: { max_jobs: 1, max_output_bytes: 64 * 1024 * 1024, max_runtime_secs: 7 * 24 * 3600 },
      registerCapability: this.options.registerCapability, environment: () => ({ ...process.env, ...options.env }),
      call: async (_op, _key, payload) => {
        if (payload.action === "prepare") await this.options.admit(resource);
        if (payload.action === "exited" && payload.group_clear === true) {
          await this.options.settle(resource); acknowledged = true;
        }
        return { state: "committed", result: {} };
      },
    });
    this.runtimes.add(runtime);
    const flush = () => {
      if (runtime.state()[0]?.bytes_total === offset) return;
      while (true) {
        const page = runtime.peek(id, { offset, max_bytes: 65536 });
        if (page.gap) options.onData(Buffer.from("\n[foreground shell output retention gap]\n"));
        if (typeof page.text === "string" && page.text) options.onData(Buffer.from(page.text));
        const next = page.next_offset;
        if (typeof next !== "number" || next <= offset) break;
        offset = next;
        if (!page.has_more) break;
      }
    };
    const unsubscribe = runtime.onLine(flush);
    // Line events stream immediately; also deliver partial lines/progress before shell exit.
    // This reads only the owned output cursor, never serves as process-settlement proof.
    const streaming = setInterval(() => { if (runtime.state().length) flush(); }, 100);
    streaming.unref();
    const abort = () => { aborted ??= runtime.kill(id); void aborted.catch(() => {}); };
    try {
      mkdirSync(directory, { recursive: true, mode: 0o700 });
      const started = await runtime.start(command, resource, options.timeout);
      if (!runtime.inspection().length && started.state === "not_started") {
        // shutdownEpoch fenced the launch before a child existed: positive not-started proof.
        await this.options.settle(resource); clean = true;
        throw new Error("assistant shell launch was fenced; no effect started");
      }
      options.signal?.addEventListener("abort", abort, { once: true });
      if (options.signal?.aborted) abort();
      await runtime.settled(id); // group sweep and all terminal-report attempts have finished
      flush();
      const result = runtime.peek(id);
      if (result.group_clear !== true || !acknowledged) throw new Error("assistant shell settlement remains uncertain; planning drain is fenced");
      clean = true;
      if (aborted) await aborted;
      if (options.signal?.aborted) throw new Error("aborted");
      if (result.termination === "deadline") throw new Error(`timeout:${options.timeout ?? 7 * 24 * 3600}`);
      const signal = typeof result.signal === "string" ? constants.signals[result.signal as NodeJS.Signals] : undefined;
      return { exitCode: typeof result.exit_code === "number" ? result.exit_code : signal === undefined ? null : 128 + signal };
    } finally {
      clearInterval(streaming); unsubscribe(); options.signal?.removeEventListener("abort", abort);
      if (clean) {
        // This one-command runtime can be collected intact. A concurrent deadline kill
        // may still be returning its final peek; do not evict the identity underneath it.
        this.runtimes.delete(runtime);
        rmSync(directory, { recursive: true, force: true });
      }
      // Unknown/lost settlement is deliberately retained, never released by a finally block.
    }
  }

  async shutdown(): Promise<void> {
    if (this.stopping) return this.stopping;
    const operation = Promise.resolve().then(async () => {
      await Promise.all([...this.runtimes].map(runtime => runtime.shutdown()));
      await Promise.allSettled([...this.pending]);
    });
    this.stopping = operation;
    try { await operation; } finally { this.stopping = undefined; }
  }

  async close(): Promise<void> {
    this.closed = true;
    await this.shutdown();
    if (this.root && !this.runtimes.size) rmSync(this.root, { recursive: true, force: true });
  }
}
