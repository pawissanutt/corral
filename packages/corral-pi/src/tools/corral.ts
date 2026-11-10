// Host-to-corral calls: one fresh `corral agent call --op <op>` process per request, with
// envelope v1 on stdin and one JSON receipt on stdout. Secrets travel only on stdin.
import { execFile, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createHash } from "node:crypto";
import { TOML } from "bun";
import { readHistory, type HistoryRequest } from "./history.ts";

import type { CodeParent } from "../extensions/codemode/child.ts";
import type { CorralOp, HostIdentity } from "../roles/types.ts";

export const ENVELOPE_VERSION = 1;
export const ENVELOPE_MAX_BYTES = 64 * 1024;
const RESPONSE_MAX_BYTES = 256 * 1024;
const CALL_TIMEOUT_MS = 60_000;

export interface CallDiagnostics {
  readonly exit_code?: number | null;
  readonly signal?: string | null;
  readonly timed_out?: boolean;
  readonly diagnostic?: string;
  /** Only a failure before spawning proves that the operation never started. */
  readonly effect?: "not_started" | "unknown";
}

export class CorralCallError extends Error {
  constructor(readonly code: string, message: string, readonly diagnostics: CallDiagnostics = {}) {
    super(`${code}: ${message}`);
  }
}

/** Object keys sorted, no whitespace: the digest input `envelope.rs` computes. */
export function canonicalJson(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) {
      throw new CorralCallError("bad_payload", "payload numbers must be safe integers");
    }
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>)
      .filter(([, item]) => item !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`).join(",")}}`;
  }
  throw new CorralCallError("bad_payload", `cannot encode a ${typeof value}`);
}

export function sha256Hex(data: string | Uint8Array): string {
  return createHash("sha256").update(data).digest("hex");
}

/** `rq-` plus 32 hex digits of SHA-256(session_id, NUL, tool_call_id). */
export function toolRequestId(sessionId: string, toolCallId: string): string {
  const digest = createHash("sha256").update(sessionId).update(new Uint8Array([0]))
    .update(toolCallId).digest("hex");
  return `rq-${digest.slice(0, 32)}`;
}

export function eventRequestId(generation: number, seq: number): string {
  return `ev-${generation}-${seq}`;
}

export interface CorralReceipt {
  readonly version: number;
  readonly request_id: string | null;
  readonly op: string;
  readonly state: "committed" | "rejected" | "reserved" | "queued" | "uncertain" | "abandoned";
  readonly result: unknown;
  readonly replayed?: boolean;
  readonly [key: string]: unknown;
}

export interface CorralCall {
  readonly requestId: string;
  readonly payload: Record<string, unknown>;
  readonly turn?: number;
  readonly bindNonce?: string;
  /** Overrides the client's deadline for this one call. */
  readonly timeoutMs?: number;
  /** A nested Code Mode call: corral checks that `requestId` is derived from it (T-683). */
  readonly parent?: CodeParent;
}

export interface CorralClientOptions {
  /** The corral executable dispatch recorded in the bootstrap. */
  readonly corral: string;
  /** The workspace root; each call runs there so corral opens the bound store. */
  readonly root: string;
  readonly identity: HostIdentity;
  readonly hostCap: string;
  readonly sessionId: () => string;
  readonly timeoutMs?: number;
}

export class CorralClient {
  constructor(private readonly options: CorralClientOptions) {}
  get workspaceRoot(): string { return this.options.root; }
  history(request: HistoryRequest): Promise<unknown> {
    return readHistory(this.options.corral, this.options.root, request);
  }

  /** Read the bound ledger on startup/resume without creating a mutation receipt. */
  todoSnapshot(signal?: AbortSignal): Promise<unknown> {
    const { identity, corral, root } = this.options;
    const args = identity.role === "pm"
      ? ["agent", "state", "--pm", "--json"]
      : identity.role === "assistant"
        // Bounded: receipt/budget/usage history must not grow startup reads past maxBuffer.
        ? ["assistant", "show", identity.assistant ?? "", "--json", "--host"]
        : ["show", identity.task ?? "", "--json"];
    return new Promise((resolve, reject) => {
      const child = execFile(corral, args, {
        cwd: root, timeout: CALL_TIMEOUT_MS, maxBuffer: 4 * 1024 * 1024, signal,
      }, (error, stdout) => {
        if (error) {
          reject(new CorralCallError("todo_read", error.message));
          return;
        }
        try {
          resolve(JSON.parse(stdout));
        } catch (error) {
          reject(new CorralCallError("todo_read", error instanceof Error ? error.message : String(error)));
        }
      });
      child.stdin?.end();
    });
  }

  /** The bound task's spec and status for the Task view; read-only, never a mutation. */
  taskSnapshot(signal?: AbortSignal): Promise<unknown> {
    const { identity, corral, root } = this.options;
    if (!identity.task) return Promise.reject(new CorralCallError("task_unbound", "No task bound to this session"));
    return new Promise((resolve, reject) => {
      const child = execFile(corral, ["show", identity.task ?? "", "--json"], {
        cwd: root, timeout: CALL_TIMEOUT_MS, maxBuffer: 4 * 1024 * 1024, signal,
      }, (error, stdout) => {
        if (error) { reject(new CorralCallError("task_read", error.message)); return; }
        try { resolve(JSON.parse(stdout)); }
        catch (error) { reject(new CorralCallError("task_read", error instanceof Error ? error.message : String(error))); }
      });
      child.stdin?.end();
    });
  }

  /** Bounded canonical read, deliberately not read_show/full task hydration or a mutation. */
  inspectionSnapshot(projection?: "lifecycle" | "artifacts"): Promise<unknown> {
    const { identity, corral, root } = this.options;
    const { task, run } = identity;
    if (!task || !run) return Promise.reject(new CorralCallError("inspection_unbound", "No task/run bound to this session"));
    return new Promise((resolve, reject) => {
      const child = execFile(corral, ["run", "inspect", task, "--run", run, ...(projection ? [`--${projection}`] : [])], {
        cwd: root, timeout: 30_000, maxBuffer: 128 * 1024,
      }, (error, stdout) => {
        if (error) { reject(new CorralCallError("inspection_read", error.message)); return; }
        try { resolve(JSON.parse(stdout)); }
        catch { reject(new CorralCallError("inspection_read", "Invalid inspection response")); }
      });
      child.stdin?.end();
    });
  }

  /**
   * An existing artifact viewer for one catalog reference, run with argv only: `show` prints bounded
   * readable text; `serve` asks corral's loopback artifact server to open the browser. Callers
   * invoke `serve` only on an explicit owner action.
   */
  artifactViewer(reference: string, viewer: "show" | "serve"): Promise<string> {
    if (!artifactReference(reference)) {
      return Promise.reject(new CorralCallError("artifact_viewer", "Invalid artifact reference"));
    }
    return new Promise((resolve, reject) => {
      const child = execFile(this.options.corral, ["--root", this.options.root, "artifact", viewer, reference], {
        cwd: this.options.root, timeout: CALL_TIMEOUT_MS, maxBuffer: 512 * 1024,
      }, (error, stdout) => {
        if (error) { reject(new CorralCallError("artifact_viewer", error.message)); return; }
        resolve(stdout);
      });
      child.stdin?.end();
    });
  }

  /** Fixed read-only inbox command at the launcher-bound workspace root, never the Git inspector scope. */
  attentionSnapshot(signal?: AbortSignal): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const child = execFile(this.options.corral, ["--root", this.options.root, "attention", "inbox", "--json"], {
        cwd: this.options.root, timeout: CALL_TIMEOUT_MS, maxBuffer: 4 * 1024 * 1024, signal,
      }, (error, stdout) => {
        if (error) { reject(new CorralCallError("inbox_read", error.message)); return; }
        try { resolve(JSON.parse(stdout)); }
        catch (error) { reject(new CorralCallError("inbox_read", error instanceof Error ? error.message : String(error))); }
      });
      child.stdin?.end();
    });
  }

  /** Read-only project/worktree choices; never infer a project from a workspace Git root. */
  async projectScopes(signal?: AbortSignal): Promise<unknown> {
    const { corral, root, identity } = this.options;
    const projects = await new Promise<unknown>((resolve, reject) => {
      const child = execFile(corral, ["--root", root, "config", "get", "projects"], {
        cwd: root, timeout: CALL_TIMEOUT_MS, maxBuffer: 256 * 1024, signal,
      }, (error, stdout) => {
        if (error) { reject(new CorralCallError("projects_read", error.message)); return; }
        try { resolve(TOML.parse(stdout)); }
        catch { reject(new CorralCallError("projects_read", "Invalid project configuration response")); }
      });
      child.stdin?.end();
    });
    const session = identity.role === "assistant" ? await this.todoSnapshot(signal) : undefined;
    return { workspace: root, projects,
      session: session && typeof session === "object" && "session" in session ? session.session : null };
  }

  envelope(op: CorralOp, call: CorralCall): string {
    const { identity } = this.options;
    const text = JSON.stringify({
      version: ENVELOPE_VERSION,
      op,
      request_id: call.requestId,
      payload_sha256: sha256Hex(canonicalJson(call.payload)),
      identity: {
        store: identity.store,
        project: identity.project,
        role: identity.role,
        task: identity.task,
        run: identity.run,
        review: identity.review,
        callback: identity.callback,
        assistant: identity.assistant,
        session_id: this.options.sessionId(),
        generation: identity.generation,
        turn: call.turn,
        parent: call.parent,
      },
      auth: { host_cap: this.options.hostCap, bind_nonce: call.bindNonce },
      payload: call.payload,
    });
    if (Buffer.byteLength(text) > ENVELOPE_MAX_BYTES) {
      throw new CorralCallError("bad_payload", `envelope exceeds ${ENVELOPE_MAX_BYTES} bytes`);
    }
    return text;
  }

  /** Narrow lifecycle seams let tests drive exit/deadline ordering without sleeps. */
  protected spawnCall(op: CorralOp): ChildProcessWithoutNullStreams {
    return spawn(this.options.corral, ["agent", "call", "--op", op], {
      cwd: this.options.root, stdio: ["pipe", "pipe", "pipe"],
    });
  }

  protected deadline(expire: () => void, ms: number): () => void {
    const timer = setTimeout(expire, ms);
    return () => clearTimeout(timer);
  }

  call(op: CorralOp, call: CorralCall): Promise<CorralReceipt> {
    const input = this.envelope(op, call);
    return new Promise((resolve, reject) => {
      const child = this.spawnCall(op);
      const out: Buffer[] = [];
      const err: Buffer[] = [];
      let size = 0;
      let settled = false;
      let failure: "call_timeout" | "call_output" | undefined;
      const finish = (error: Error | undefined, receipt?: CorralReceipt) => {
        if (settled) return;
        settled = true;
        cancelDeadline();
        if (error) reject(error);
        else if (receipt) resolve(receipt);
      };
      const destroyPipes = () => { child.stdout.destroy(); child.stderr.destroy(); };
      const stop = (reason: "call_timeout" | "call_output") => {
        failure ??= reason;
        child.kill("SIGKILL");
        // Exit may have happened before this bound was reached. No second exit event
        // will arrive in that ordering, so release descendant-held pipes here as well.
        if (child.exitCode !== null || child.signalCode !== null) destroyPipes();
      };
      const cancelDeadline = this.deadline(() => stop("call_timeout"),
        call.timeoutMs ?? this.options.timeoutMs ?? CALL_TIMEOUT_MS);
      const collect = (into: Buffer[]) => (chunk: Buffer) => {
        const remaining = Math.max(0, RESPONSE_MAX_BYTES - size);
        size += chunk.length;
        if (remaining > 0) into.push(chunk.subarray(0, remaining));
        if (size > RESPONSE_MAX_BYTES) {
          stop("call_output");
        }
      };
      child.stdout.on("data", collect(out));
      child.stderr.on("data", collect(err));
      const redact = (text: string) => [this.options.hostCap, call.bindNonce].reduce<string>(
        (s, secret) => secret ? s.replaceAll(secret, "[redacted]") : s, text,
      ).slice(-4096);
      child.on("error", (error) => finish(new CorralCallError(
        child.pid === undefined ? "call_spawn" : "call_failed", redact(error.message),
        { effect: child.pid === undefined ? "not_started" : "unknown" },
      )));
      child.on("exit", () => {
        // A descendant can retain the executor's pipe descriptors. Once a bounded call
        // was killed, do not let those descriptors turn its deadline into an infinite wait.
        if (failure) destroyPipes();
      });
      // EPIPE may race a process exit; the close event retains the actual status and diagnostics.
      child.stdin.on("error", () => {});
      child.on("close", (code, signal) => {
        const text = Buffer.concat(out).toString("utf8").trim();
        // A canonical response is stronger evidence than an exit status, including a receipt
        // flushed before the process was killed. Never synthesize a rejection from an exit.
        try {
          const receipt = parseReceipt(text);
          const lookup = receipt.lookup as { by?: unknown } | undefined;
          if ((receipt.op === op && receipt.request_id === call.requestId)
            || (op === "receipt_lookup" && lookup?.by === call.requestId)) {
            finish(undefined, receipt);
            return;
          }
        } catch { /* Report bounded transport diagnostics below, not untrusted JSON text. */ }
        const diagnostic = redact(Buffer.concat(err).toString("utf8").trim());
        const errorCode = failure ?? (code !== 0 || signal ? "call_failed" : "call_output");
        finish(new CorralCallError(errorCode,
          `corral agent call --op ${op}: exit=${code}, signal=${signal}, timed_out=${failure === "call_timeout"}; ${diagnostic}`,
          { exit_code: code, signal, timed_out: failure === "call_timeout", diagnostic, effect: "unknown" }));
      });
      child.stdin.end(input);
    });
  }
}

function parseReceipt(text: string): CorralReceipt {
  if (text.includes("\n")) throw new CorralCallError("call_output", "expected exactly one JSON receipt line");
  const value: unknown = JSON.parse(text);
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new CorralCallError("call_output", "the receipt is not a JSON object");
  }
  const receipt = value as CorralReceipt;
  if (receipt.version !== 1 || !(typeof receipt.request_id === "string" || receipt.request_id === null)
    || typeof receipt.op !== "string" || !Object.hasOwn(receipt, "result")
    || !["committed", "rejected", "reserved", "queued", "uncertain", "abandoned"].includes(receipt.state)) {
    throw new CorralCallError("call_output", "the response is not a v1 Corral receipt");
  }
  return receipt;
}

/**
 * Corral's artifact reference grammar (`PinRef::parse`): `T-<n>:<run>:<file>` or `A-<n>:<file>`. Stored
 * file names keep their source spelling (spaces, Unicode, `:` after the run), so only structure is
 * checked; the reference is passed as one argv element, never through a shell. Control characters,
 * path separators and names `PinRef::parse` would trim are refused.
 */
export function artifactReference(reference: string): boolean {
  const match = /^(?:T-\d+:[A-Za-z0-9]+|A-\d+):(.+)$/su.exec(reference);
  const file = match?.[1];
  if (file === undefined || file !== file.trim() || file.length > 1024 || /[\u0000-\u001f\u007f/\\]/u.test(file)) return false;
  return reference.startsWith("T-") || !file.includes(":");
}
