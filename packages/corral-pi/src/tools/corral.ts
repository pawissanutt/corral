// Host-to-corral calls: one fresh `corral agent call --op <op>` process per request, with
// envelope v1 on stdin and one JSON receipt on stdout. Secrets travel only on stdin.
import { execFile, spawn } from "node:child_process";
import { createHash } from "node:crypto";

import type { CorralOp, HostIdentity } from "../roles/types.ts";

export const ENVELOPE_VERSION = 1;
export const ENVELOPE_MAX_BYTES = 64 * 1024;
const RESPONSE_MAX_BYTES = 256 * 1024;
const CALL_TIMEOUT_MS = 60_000;

export class CorralCallError extends Error {
  constructor(readonly code: string, message: string) {
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

  /** Read the bound ledger on startup/resume without creating a mutation receipt. */
  todoSnapshot(): Promise<unknown> {
    const { identity, corral, root } = this.options;
    const args = identity.role === "pm"
      ? ["agent", "state", "--pm", "--json"]
      : ["show", identity.task ?? "", "--json"];
    return new Promise((resolve, reject) => {
      const child = execFile(corral, args, {
        cwd: root, timeout: CALL_TIMEOUT_MS, maxBuffer: 4 * 1024 * 1024,
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
        session_id: this.options.sessionId(),
        generation: identity.generation,
        turn: call.turn,
      },
      auth: { host_cap: this.options.hostCap, bind_nonce: call.bindNonce },
      payload: call.payload,
    });
    if (Buffer.byteLength(text) > ENVELOPE_MAX_BYTES) {
      throw new CorralCallError("bad_payload", `envelope exceeds ${ENVELOPE_MAX_BYTES} bytes`);
    }
    return text;
  }

  call(op: CorralOp, call: CorralCall): Promise<CorralReceipt> {
    const input = this.envelope(op, call);
    return new Promise((resolve, reject) => {
      const child = spawn(this.options.corral, ["agent", "call", "--op", op], {
        cwd: this.options.root,
        stdio: ["pipe", "pipe", "pipe"],
      });
      const out: Buffer[] = [];
      const err: Buffer[] = [];
      let size = 0;
      let settled = false;
      const finish = (error: Error | undefined, receipt?: CorralReceipt) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (error) reject(error);
        else if (receipt) resolve(receipt);
      };
      const timer = setTimeout(() => {
        child.kill("SIGKILL");
        finish(new CorralCallError("call_timeout", `corral agent call --op ${op} exceeded its deadline`));
      }, this.options.timeoutMs ?? CALL_TIMEOUT_MS);
      const collect = (into: Buffer[]) => (chunk: Buffer) => {
        size += chunk.length;
        if (size > RESPONSE_MAX_BYTES) {
          child.kill("SIGKILL");
          finish(new CorralCallError("call_output", "corral agent call output exceeds its bound"));
          return;
        }
        into.push(chunk);
      };
      child.stdout.on("data", collect(out));
      child.stderr.on("data", collect(err));
      child.on("error", (error) => finish(new CorralCallError("call_spawn", error.message)));
      child.on("close", (code) => {
        const text = Buffer.concat(out).toString("utf8").trim();
        if (code !== 0) {
          const detail = Buffer.concat(err).toString("utf8").trim().split("\n").slice(-3).join(" ");
          finish(new CorralCallError("call_failed", `corral agent call exited ${code}: ${detail}`));
          return;
        }
        try {
          finish(undefined, parseReceipt(text));
        } catch (error) {
          finish(error instanceof Error ? error : new CorralCallError("call_output", String(error)));
        }
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
  if (typeof receipt.state !== "string" || typeof receipt.op !== "string") {
    throw new CorralCallError("call_output", "the receipt lacks state or op");
  }
  return receipt;
}
