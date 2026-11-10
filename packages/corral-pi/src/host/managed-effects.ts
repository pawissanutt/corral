// Managed executor facts, not workflow authority. Journal before canonical publication;
// never infer old-executor termination from an empty replacement manager.
import { randomUUID } from "node:crypto";
import { StdioTransport } from "@earendil-works/pi-mcp";
import type { ChildProcess } from "node:child_process";
import { readFileSync } from "node:fs";
import { processGroup, startTimeOf } from "../extensions/jobs/runtime.ts";
import { sha256Hex } from "../tools/corral.ts";
import type { RpcOutcome } from "./managed-rpc.ts";

export class OwnedStdio extends StdioTransport {
  private observedClose = false;
  private stopped: Promise<void> | undefined;
  constructor(options: ConstructorParameters<typeof StdioTransport>[0], private readonly owner: ManagedInstance) {
    super(options); this.onClose(() => { this.observedClose = true; });
  }
  override async start(): Promise<void> {
    await this.owner.prepare();
    await super.start();
    if (this.pid === undefined) throw new Error("MCP stdio has no owned process identity");
    await this.owner.bindProcess(this.pid);
  }
  override close(): Promise<void> {
    return this.stopped ??= (async () => {
      if (!this.owner.process) {
        await super.close(); // owning SDK handle cleanup only; never a terminal attestation
        if (!this.owner.protected) return;
        throw new Error("MCP owned process identity unavailable; shutdown unknown");
      }
      if (!this.observedClose) await new Promise<void>((resolve, reject) => {
        const unsubscribe = this.onClose(() => { clearTimeout(timer); clearTimeout(kill); unsubscribe(); resolve(); });
        const signal = (name: NodeJS.Signals) => {
          try { this.owner.signal(name); }
          catch (error) { clearTimeout(timer); clearTimeout(kill); unsubscribe(); reject(error); }
        };
        const kill = setTimeout(() => signal("SIGKILL"), 500);
        const timer = setTimeout(() => { clearTimeout(kill); unsubscribe(); reject(new Error("owned MCP transport exit unproved")); }, 2_000);
        signal("SIGTERM");
      });
      // Only after its real close event has cleared the SDK child handle. Never let the
      // SDK's post-exit negative-PID fallback signal a no-longer-owned/reused group.
      await super.close();
      await this.owner.verifyProcess();
    })();
  }
}

export type Executor = { kind: "preparing"; resource: string; config_sha256: string }
  | { kind: "process"; resource: string; config_sha256: string; process: { pid: number; start_time: string; boot_id: string } }
  | { kind: "http"; resource: string; config_sha256: string; connection: string };
export interface Terminal {
  disposition: "completed" | "shutdown_verified" | "not_started";
  executor_sha256: string; exit_observed: boolean; group_clear: boolean; requests_terminal: boolean;
  outcome: string; delivery: string; requests: RpcOutcome[];
}
export type Lifecycle = { action: "prepare" | "bind"; instance: string; executor: Executor }
  | { action: "rpc_prepared"; instance: string; id: string; method: string }
  | { action: "rpc_unknown"; instance: string; reason: string; requests: RpcOutcome[] }
  | { action: "terminal"; instance: string; terminal: Terminal } | { action: "seal" };
export type EvidenceSink = (event: Lifecycle) => Promise<void>;

// Rust serde field order is fixed; explicitly canonicalize instead of depending on property
// insertion order or on ambient SDK serialization. This digest binds terminal to executor.
export function executorDigest(executor: Executor): string {
  const common = { kind: executor.kind, resource: executor.resource, config_sha256: executor.config_sha256 };
  return sha256Hex(JSON.stringify(executor.kind === "process" ? { ...common, process: {
    pid: executor.process.pid, start_time: executor.process.start_time, boot_id: executor.process.boot_id,
  } } : executor.kind === "http" ? { ...common, connection: executor.connection } : common));
}
export function terminal(executor: Executor, disposition: Terminal["disposition"], outcome: string,
  requests: RpcOutcome[] = []): Terminal {
  return { disposition, executor_sha256: executorDigest(executor), exit_observed: executor.kind === "process",
    group_clear: executor.kind === "process", requests_terminal: requests.every(request => request.state !== "unknown"),
    outcome, delivery: "lifecycle_journal", requests };
}

export class ManagedInstance {
  readonly token = randomUUID();
  private executor: Executor;
  private preparation: Promise<void> | undefined;
  private publication: Promise<void> | undefined;
  constructor(resource: string, config: unknown, private readonly sink?: EvidenceSink) {
    this.executor = { kind: "preparing", resource, config_sha256: sha256Hex(JSON.stringify(config)) };
  }
  get protected(): boolean { return this.sink !== undefined; }
  prepare(): Promise<void> {
    return this.preparation ??= this.emit({ action: "prepare", instance: this.token, executor: this.executor });
  }
  async bindProcess(pid: number): Promise<void> {
    const start = startTimeOf(pid);
    if (process.platform !== "linux" || !start || start === "0") {
      if (this.protected) throw new Error("executor process identity unsupported/unknown; protected drain unavailable");
      return; // ordinary unprotected manager cleanup, never emits executor proof
    }
    const boot = readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim();
    await this.bind({ kind: "process", resource: this.executor.resource, config_sha256: this.executor.config_sha256,
      process: { pid, start_time: start, boot_id: boot } });
  }
  async bindHttp(): Promise<void> {
    await this.bind({ kind: "http", resource: this.executor.resource, config_sha256: this.executor.config_sha256, connection: this.token });
  }
  private async bind(executor: Executor): Promise<void> {
    await this.prepare();
    if (this.executor.kind !== "preparing") throw new Error("executor instance already bound");
    this.executor = executor; // retain the concrete handle even if publication is interrupted
    await this.emit({ action: "bind", instance: this.token, executor });
  }
  private emit(event: Lifecycle): Promise<void> { return this.sink?.(event) ?? Promise.resolve(); }
  snapshot(): { instance: string; executor: Executor } { return { instance: this.token, executor: structuredClone(this.executor) }; }
  get process(): { pid: number; start_time: string; boot_id: string } | undefined {
    return this.executor.kind === "process" ? this.executor.process : undefined;
  }
  async verifyProcess(): Promise<void> {
    const identity = this.process;
    if (!identity) throw new Error("owned process identity missing; shutdown unknown");
    const deadline = Date.now() + 2_000;
    for (;;) {
      const group = processGroup(identity.pid, identity.start_time);
      if (group.reused) throw new Error("executor PID/group identity changed; shutdown unknown");
      if (!group.members) return;
      if (Date.now() >= deadline) throw new Error("owned executor group remains; shutdown incomplete");
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  }
  signal(signal: NodeJS.Signals): void {
    const identity = this.process;
    if (!identity || !processGroup(identity.pid, identity.start_time).owned) return;
    try { process.kill(-identity.pid, signal); } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
    }
  }
  // Caller must have observed the owned transport/child exit, not merely sent a signal.
  async rpcPrepared(id: string, method: string): Promise<void> {
    await this.emit({ action: "rpc_prepared", instance: this.token, id, method });
  }
  async rpcUnknown(reason: string, requests: RpcOutcome[]): Promise<void> {
    await this.emit({ action: "rpc_unknown", instance: this.token, reason, requests });
  }
  async stopped(outcome: string, requests: RpcOutcome[] = []): Promise<void> {
    if (this.executor.kind === "process") await this.verifyProcess();
    if (this.executor.kind === "preparing") {
      if (!this.protected) return;
      throw new Error("unbound executor; shutdown unknown");
    }
    this.publication ??= this.emit({ action: "terminal", instance: this.token,
      terminal: terminal(this.executor, "shutdown_verified", outcome, requests) });
    await this.publication;
  }
  async notStarted(): Promise<void> {
    if (this.executor.kind !== "preparing") throw new Error("started executor cannot become not-started");
    await this.prepare();
    this.publication ??= this.emit({ action: "terminal", instance: this.token,
      terminal: terminal(this.executor, "not_started", "launch fence: no executor created") });
    await this.publication;
  }
}

export function childExit(child: ChildProcess, timeoutMs = 2_000): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const finish = () => { clearTimeout(timer); resolve(); };
    const timer = setTimeout(() => { child.off("exit", finish); reject(new Error("owned child exit unproved")); }, timeoutMs);
    child.once("exit", finish);
  });
}

/** Shared by explicit close, entry and SDK shutdown. A failure is retained, never hidden by
 * another disposal callback. Shutdown and canonical acknowledgement are NOT retried as repair. */
export class ManagedEffects {
  private readonly shutdowns = new Map<string, Promise<void>>();
  private readonly closed = new Set<string>();
  private readonly completed = new Set<string>();
  constructor(private readonly seal: (resource: string) => Promise<void>,
    private readonly release: (resource: string) => Promise<void>) {}
  requireOpen(resource: string): void {
    if (this.closed.has(resource)) throw new Error(`${resource} manager stopped; no effect started`);
  }
  /** Only explicit SDK runtime replacement, after verified settlement, opens a NEW manager.
   * Old instances remain permanently stopped. Failed/unknown shutdown cannot be replaced. */
  newLifetime(resource: string): boolean {
    if (!this.closed.has(resource)) return false;
    if (!this.completed.has(resource)) throw new Error(`${resource} prior lifecycle remains unknown; replacement refused`);
    this.closed.delete(resource); this.completed.delete(resource); this.shutdowns.delete(resource);
    return true;
  }
  stop(resource: string, shutdown: () => Promise<void>): Promise<void> {
    this.closed.add(resource); // synchronous admission barrier, before the first await
    let pending = this.shutdowns.get(resource);
    if (!pending) {
      pending = (async () => { await shutdown(); await this.seal(resource); await this.release(resource); this.completed.add(resource); })();
      this.shutdowns.set(resource, pending);
    }
    return pending;
  }
  async stopAll(resources: readonly [string, () => Promise<void>][]): Promise<void> {
    const results = await Promise.allSettled(resources.map(([name, shutdown]) => this.stop(name, shutdown)));
    const errors = results.flatMap(r => r.status === "rejected" ? [r.reason] : []);
    if (errors.length) throw new AggregateError(errors, "managed resource shutdown/settlement incomplete");
  }
}
