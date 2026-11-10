// Exact owned MCP request observations. A JSON-RPC response ID alone is not terminal
// evidence: malformed results and asynchronous task acknowledgements can leave work unknown.
import { AsyncLocalStorage } from "node:async_hooks";
import { McpError, type JsonRpcMessage } from "@earendil-works/pi-mcp";
import { sha256Hex } from "../tools/corral.ts";

export interface RpcOutcome {
  readonly id: string; readonly method: string;
  readonly state: "validated_result" | "rpc_error" | "not_started" | "unknown";
  readonly reply_sha256: string | null; readonly error_code: number | null;
  readonly delivery: "adapter_observed" | "runtime_fence" | "unknown";
  readonly remote_task_id: string | null;
}
type Request = {
  method: string;
  replySha256?: string;
  response?: "result" | "error" | "nonterminal";
  error?: { code: number; message: string; data?: unknown };
  terminal: boolean;
  dispatched: boolean;
  notStarted?: boolean;
  remoteTask?: string;
};
// MCP JSON schemas/results legitimately contain floating-point JSON numbers. Do not use
// Corral's integer-only authority-payload codec to fingerprint an MCP protocol response.
function canonicalRpc(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalRpc);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
    .map(([key, item]) => [key, canonicalRpc(item)]));
}
export class ManagedRpc {
  private readonly context = new AsyncLocalStorage<Set<string>>();
  private readonly requests = new Map<string, Request>();
  private uncertain = false;
  private readonly fences = new WeakMap<Error, ReadonlySet<string>>();
  fence(error: unknown): Error {
    const failure = new Error(error instanceof Error ? error.message : String(error), { cause: error });
    const action = this.context.getStore();
    if (action) this.fences.set(failure, action);
    return failure;
  }
  get unknown(): boolean { return this.uncertain || [...this.requests.values()].some(request => !request.terminal); }
  get settled(): boolean { return !this.uncertain && [...this.requests.values()].every(request => request.terminal); }
  outcomes(): RpcOutcome[] {
    return [...this.requests].map(([id, request]) => ({ id, method: request.method,
      state: request.notStarted ? "not_started" : !request.terminal ? "unknown" : request.response === "error" ? "rpc_error" : "validated_result",
      reply_sha256: request.replySha256 ?? null,
      error_code: request.terminal && request.response === "error" ? request.error?.code ?? null : null,
      delivery: request.notStarted ? "runtime_fence" : request.terminal ? "adapter_observed" : "unknown",
      remote_task_id: request.remoteTask ?? null }));
  }
  prepare(message: JsonRpcMessage): string | undefined {
    if (!("id" in message) || !("method" in message)) return undefined;
    const id = String(message.id), action = this.context.getStore();
    if (!action || this.requests.has(id) || this.requests.size >= 128) {
      throw new Error("MCP request identity unavailable/reused/full; no request dispatched");
    }
    action.add(id); this.requests.set(id, { method: message.method, terminal: false, dispatched: false });
    return id;
  }
  dispatch(id: string): void {
    const request = this.requests.get(id);
    if (!request || request.dispatched || request.notStarted) throw new Error("MCP dispatch identity changed");
    request.dispatched = true;
  }
  notStarted(id: string): void {
    const request = this.requests.get(id);
    if (!request || request.dispatched || request.response) { this.uncertain = true; return; }
    request.notStarted = true; request.terminal = true;
  }
  received(message: JsonRpcMessage): void {
    if (!("id" in message) || "method" in message) return;
    const request = this.requests.get(String(message.id));
    if (!request) return;
    if (!request.dispatched) this.uncertain = true;
    const digest = sha256Hex(JSON.stringify(canonicalRpc(message)));
    if (request.response) { if (request.replySha256 !== digest) this.uncertain = true; return; }
    request.replySha256 = digest;
    if ("error" in message) {
      request.response = "error"; request.error = message.error;
    } else {
      const result = message.result;
      // Task support is not implemented by this manager. An acknowledgement is not proof
      // the remote executor finished, even if the pinned client defaults missing content.
      const object = result !== null && typeof result === "object" && !Array.isArray(result);
      const task = object && "task" in result;
      if (task) {
        const id = (result as { task?: { taskId?: unknown } }).task?.taskId;
        if (typeof id === "string" && id.length > 0 && id.length <= 256 && !/[\x00-\x1f\x7f]/u.test(id)) request.remoteTask = id;
      }
      const unknownToolShape = request.method === "tools/call" && (!object
        || (!Array.isArray((result as { content?: unknown }).content) && !("structuredContent" in result)));
      request.response = task || unknownToolShape ? "nonterminal" : "result";
    }
  }
  async observe<T>(action: () => Promise<T>): Promise<T> {
    const ids = new Set<string>();
    try {
      const result = await this.context.run(ids, action);
      this.finish(ids, true);
      return result;
    } catch (error) {
      const last = this.requests.get([...ids].at(-1) ?? "");
      // A matching protocol error is a known failed result, not an unknown execution.
      // SDK result-validation errors do not have a matching wire error and stay unknown.
      const knownError = (error instanceof Error && this.fences.get(error) === ids)
        || (error instanceof McpError && last?.response === "error"
          && last.error?.code === error.code && last.error.message === error.message && last.error.data === error.data);
      this.finish(ids, knownError);
      throw error;
    }
  }
  private finish(ids: ReadonlySet<string>, validated: boolean): void {
    for (const id of ids) {
      const request = this.requests.get(id);
      if (!request) { this.uncertain = true; continue; }
      if (request.notStarted) continue; // positive pre-dispatch fence, even after caller abort
      request.terminal = validated && request.dispatched && ["result", "error"].includes(request.response ?? "");
    }
  }
}
