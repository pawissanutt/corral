import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import type { CorralOp, HostIdentity } from "../roles/types.ts";
import { CorralClient, CorralCallError, sha256Hex, type CorralCall, type CorralReceipt } from "../tools/corral.ts";

export type LocalOpHandler = (call: CorralCall) => unknown | Promise<unknown>;
export class LocalRefusal extends Error {
  constructor(readonly code: string, reason: string) { super(reason); }
}
interface LocalOptions {
  readonly dir: string;
  readonly root: string;
  readonly identity: HostIdentity;
  readonly sessionId: () => string;
  readonly handlers?: Readonly<Partial<Record<CorralOp, LocalOpHandler>>>;
  readonly todoSnapshot?: () => unknown;
}

/** A build host has receipts but no store binding and never invokes a corral process. */
export class LocalControlPlane extends CorralClient {
  private readonly receipts = new Map<string, CorralReceipt>();
  private readonly pending = new Map<string, Promise<CorralReceipt>>();
  private readonly reservations = new Map<string, { tokens: number; usd_nanos: number }>();
  private readonly settled = new Set<string>();
  private tokens = 0;
  private usdNanos = 0;
  constructor(private readonly local: LocalOptions) {
    super({ corral: "", root: local.root, identity: local.identity, hostCap: "", sessionId: local.sessionId });
    mkdirSync(local.dir, { recursive: true, mode: 0o700 });
  }
  override get workspaceRoot(): string { return this.local.root; }
  override history(_request: Parameters<CorralClient["history"]>[0]): Promise<unknown> {
    return Promise.reject(new CorralCallError("history_read", "History is unavailable in a build session"));
  }
  override async todoSnapshot(_signal?: AbortSignal): Promise<unknown> {
    return this.local.todoSnapshot?.() ?? { container: { state: { todo: null, todo_requests: null } } };
  }
  override taskSnapshot(_signal?: AbortSignal): Promise<unknown> {
    return Promise.reject(new CorralCallError("task_unbound", "No task bound to this session"));
  }
  override inspectionSnapshot(_projection?: "lifecycle" | "artifacts"): Promise<unknown> {
    return Promise.reject(new CorralCallError("inspection_unbound", "No task/run bound to this session"));
  }
  override artifactViewer(_reference: string, _viewer: "show" | "serve"): Promise<string> {
    return Promise.reject(new CorralCallError("artifact_viewer", "Artifact viewing is unavailable in a build session"));
  }
  override async attentionSnapshot(_signal?: AbortSignal): Promise<unknown> {
    return { version: 1, workspace: this.local.root, observed_at: new Date().toISOString(), items: [] };
  }
  override async projectScopes(_signal?: AbortSignal): Promise<unknown> {
    return { workspace: this.local.root, projects: {}, session: null };
  }
  override envelope(op: CorralOp, call: CorralCall): string { return super.envelope(op, call); }

  override async call(op: CorralOp, call: CorralCall): Promise<CorralReceipt> {
    const previous = this.pending.get(call.requestId);
    let receipt: CorralReceipt | undefined;
    try {
      if (previous) receipt = { ...await previous, replayed: true };
      else {
        // Defer execution until the promise is registered, including synchronous handlers.
        const pending = Promise.resolve().then(() => this.dispatch(op, call));
        this.pending.set(call.requestId, pending);
        receipt = await pending;
        this.receipts.set(call.requestId, receipt);
      }
      return receipt;
    } finally {
      appendFileSync(join(this.local.dir, "journal.jsonl"), `${JSON.stringify({ time: new Date().toISOString(), op,
        request_id: call.requestId, ...(op === "event" && typeof call.payload.kind === "string" ? { event_kind: call.payload.kind } : {}),
        state: receipt?.state ?? "failed" })}\n`, { mode: 0o600 });
    }
  }

  private async dispatch(op: CorralOp, call: CorralCall): Promise<CorralReceipt> {
    const base = { version: 1, request_id: call.requestId, op };
    try {
      const handler = this.local.handlers?.[op];
      if (handler) return { ...base, state: "committed", result: await handler(call) };
      if (op === "receipt_lookup") {
        const target = String(call.payload.request_id);
        const found = this.receipts.get(target);
        return { ...(found ?? { ...base, state: "committed" as const, result: { found: false } }),
          lookup: { by: call.requestId, request_id: target, found: found !== undefined } };
      }
      if (op === "hello" || op === "job" || op === "job_monitor") return { ...base, state: "committed", result: {} };
      if (op === "event") return { ...base, state: "committed", result: this.event(call) };
      throw new LocalRefusal("unsupported_in_build", `${op} is unavailable in a build session`);
    } catch (error) {
      const code = error instanceof Error && "code" in error && typeof error.code === "string" ? error.code : "local_fault";
      return { ...base, state: "rejected", result: { code, reason: error instanceof Error ? error.message : String(error) } };
    }
  }

  private event(call: CorralCall): unknown {
    const payload = call.payload;
    if (payload.kind === "owner_input") return { input: `I-${sha256Hex(`${this.local.sessionId()}\0${call.requestId}`).slice(0, 24)}` };
    if (payload.kind === "auxiliary" && payload.action === "prepare") {
      if (typeof payload.request !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(payload.request)) {
        throw new LocalRefusal("bad_payload", "Invalid auxiliary request id");
      }
      const dir = join(this.local.dir, "auxiliary");
      mkdirSync(dir, { recursive: true, mode: 0o700 });
      return { session_id: payload.request, session_file: join(dir, `${payload.request}.jsonl`) };
    }
    if (payload.kind !== "budget") return {};
    const request = String(payload.request);
    const amount = { tokens: typeof payload.tokens === "number" ? payload.tokens : 0,
      usd_nanos: typeof payload.usd_nanos === "number" ? payload.usd_nanos : 0 };
    if (payload.action === "reserve" && !this.reservations.has(request) && !this.settled.has(request)) this.reservations.set(request, amount);
    if (payload.action === "settle" && !this.settled.has(request)) {
      this.reservations.delete(request);
      this.settled.add(request);
      this.tokens += amount.tokens;
      this.usdNanos += amount.usd_nanos;
    }
    const reserved = [...this.reservations.values()].reduce((sum, value) => ({ tokens: sum.tokens + value.tokens,
      usd_nanos: sum.usd_nanos + value.usd_nanos }), { tokens: 0, usd_nanos: 0 });
    return { admitted: true, exhausted: false, guard_paused: false, limits: { max_tokens: null, max_usd_nanos: null },
      used: { tokens: this.tokens, usd_nanos: this.usdNanos }, reserved };
  }
}
