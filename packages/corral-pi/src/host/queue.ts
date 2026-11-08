// Queue authority lives at Pi's synchronous object-mutation boundary, never in a second queue.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AgentSession } from "@earendil-works/pi-coding-agent";
import type { ImageContent } from "@earendil-works/pi-ai";
import { BridgeRefusal, INPUT_MAX_BYTES, type SubmitInput, type SubmitResult } from "../bridge/protocol.ts";
import { canonicalJson, sha256Hex } from "../tools/corral.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";

export const QUEUE_ENTRY = "corral_queue";
export const QUEUE_OPERATION_ENTRY = "corral_queue_operation";
export const QUEUE_OPERATIONS_MAX = 1024;
const SNAPSHOT_BYTES = 48 * 1024;
type Mode = "prompt" | "steer" | "follow_up";
type State = "queued" | "started" | "withdrawn" | "handled" | "rejected";
type Origin = "pane" | "corral-inbox" | "extension";
type Observer = NonNullable<Parameters<AgentSession["setQueueObserver"]>[0]>;
interface Item {
  readonly item: string;
  readonly origin: Origin;
  readonly text: string;
  readonly images?: ImageContent[];
  readonly text_sha256: string;
  readonly text_bytes: number;
  readonly idempotency_key?: string;
  readonly supersedes?: string;
  mode: Mode;
  state?: State;
  message?: AgentMessage;
  turn?: number;
  first_op_key?: string;
}
export interface QueueOutcome {
  readonly op_key: string;
  readonly item?: string;
  readonly outcome: "withdrawn" | "already_started" | "already_withdrawn" | "unknown_item";
  readonly turn?: number;
  readonly first_op_key?: string;
  readonly replayed: boolean;
  readonly replacement?: SubmitResult & { item: string };
}
interface Operation {
  readonly digest: string;
  result?: QueueOutcome;
  replacement?: { item: string; disposition: SubmitResult };
}
export interface HostQueueOptions {
  readonly generation: number;
  readonly session: () => AgentSession;
  readonly enabled: () => boolean;
  readonly interactive: () => boolean;
  readonly current: () => boolean;
  readonly turn: (nextRun: boolean) => number;
  readonly emit: (kind: string, data: Record<string, unknown>) => void;
  readonly fence: () => void;
}

export class HostQueue {
  private binding: { session_id: string; bind: string } | undefined;
  private counter = 0;
  private operationCounter = 0;
  private failure: string | undefined;
  private readonly items = new Map<string, Item>();
  private readonly keys = new Map<string, Item>();
  private readonly objects = new WeakMap<AgentMessage, Item>();
  private readonly pending = new Set<Item>();
  private readonly operations = new Map<string, Operation>();
  // The cause applies only in the synchronous remove call: it never spans an await.
  private removal: { op_key: string; actor: "corral" | "pane"; reason: string } | undefined;
  constructor(private readonly options: HostQueueOptions) {}

  bind(session_id: string, bind: string): void {
    if (this.binding && (this.binding.session_id !== session_id || this.binding.bind !== bind)) {
      throw new BridgeRefusal("stale_item", "queue cannot adopt another binding");
    }
    this.binding = { session_id, bind };
  }

  readonly observer: Observer = {
    input: input => this.binding ? this.mint(input.text, input.mode, input.source === "interactive" ? "pane" : "extension", input.images) : undefined,
    lifecycle: ({ tag, event, reason }) => {
      const item = this.tagged(tag);
      if (!item || event === "queued" || item.state !== undefined) return;
      this.record(item, event, event === "started" ? { turn: this.options.turn(true) } : { reason });
    },
    queue: change => {
      this.healthy();
      for (const entry of change.entries) {
        let item = this.tagged(entry.tag);
        if (change.action === "enqueue") {
          // Direct extension/custom messages bypass input handlers, but are still journaled.
          item ??= this.mint(messageText(entry.message), change.mode, "extension");
          item.mode = change.mode;
          this.record(item, "queued");
          item.message = entry.message;
          this.objects.set(entry.message, item);
        } else {
          item ??= this.objects.get(entry.message);
          if (!item || item.state !== "queued") throw new BridgeRefusal("queue_unknown", "unjournaled queue object; progress fenced");
          this.record(item, change.action === "drain" ? "started" : "withdrawn",
            change.action === "drain" ? { turn: this.options.turn(change.nextRun) }
              : this.removal ?? { actor: change.cause === "pane_restore" ? "pane_restore" : "host", reason: change.cause ?? "clear" });
          item.message = undefined;
        }
      }
    },
    canRestore: tag => this.tagged(tag)?.origin === "pane",
  };

  /** Allocated before asynchronous bridge preflight, retaining the original envelope digest. */
  bridgeInput(input: SubmitInput): unknown {
    return this.mint(input.text, input.busyPolicy, input.origin === "corral-inbox" ? "corral-inbox" : "extension", undefined, input.idempotencyKey);
  }

  private mint(text: string, mode: Mode, origin: Origin, images?: ImageContent[], key?: string, supersedes?: string): Item {
    this.healthy();
    const binding = this.binding;
    if (!binding) throw new BridgeRefusal("host_not_bound", "queue has no committed binding");
    const item: Item = { item: `q-g${this.options.generation}-${binding.bind.slice(0, 12)}-${++this.counter}`,
      origin, mode, text, text_sha256: sha256Hex(text), text_bytes: Buffer.byteLength(text),
      ...(images ? { images } : {}), ...(key ? { idempotency_key: key } : {}), ...(supersedes ? { supersedes } : {}) };
    this.items.set(item.item, item);
    if (key) this.keys.set(key, item);
    return item;
  }

  private tagged(tag: unknown): Item | undefined {
    if (typeof tag !== "object" || tag === null || !("item" in tag) || typeof tag.item !== "string") return undefined;
    const item = this.items.get(tag.item);
    return item === tag ? item : undefined;
  }

  private healthy(): void {
    if (this.failure) throw new BridgeRefusal("queue_persistence_failed", this.failure);
  }
  private append(type: string, data: Record<string, unknown>): void {
    this.healthy();
    try {
      this.options.session().sessionManager.appendCustomEntry(type, data, { immediate: true });
    } catch (error) {
      this.failure = String(error).slice(0, 256);
      this.options.fence();
      throw new BridgeRefusal("queue_persistence_failed", this.failure);
    }
  }
  private record(item: Item, event: State, extra: Record<string, unknown> = {}): void {
    const pane = utf8Prefix(item.text, INPUT_MAX_BYTES);
    const data = { version: 1, ...this.binding, generation: this.options.generation, item: item.item, event,
      origin: item.origin, mode: item.mode, text_sha256: item.text_sha256, text_bytes: item.text_bytes,
      ...(item.idempotency_key ? { idempotency_key: item.idempotency_key } : {}),
      ...(item.supersedes ? { supersedes: item.supersedes } : {}),
      ...(item.origin === "pane" ? { text: pane, ...(pane !== item.text ? { truncated: true } : {}) } : {}), ...extra };
    this.append(QUEUE_ENTRY, data);
    item.state = event;
    if (event === "queued") this.pending.add(item); else this.pending.delete(item);
    if (typeof extra.turn === "number") item.turn = extra.turn;
    if (typeof extra.op_key === "string") item.first_op_key = extra.op_key;
    // Publish only after the synchronous core mutation has returned. Public subscribers must
    // not reenter a half-mutated queue; the durable hook above remains the linearization point.
    queueMicrotask(() => this.options.emit("queue_item", data));
  }

  private check(): void {
    this.healthy();
    if (!this.binding || !this.options.current()) throw new BridgeRefusal("stale_item", "queue binding is not current");
    if (!this.options.enabled()) throw new BridgeRefusal("capability_unavailable", "queue_control is unavailable");
    if (!this.options.interactive()) throw new BridgeRefusal("not_interactive", "queue controls require an interactive host");
  }

  /** Metadata only over the bridge; text is returned solely to the interactive host API. */
  snapshot(limit = 64, local = true): Record<string, unknown> {
    this.check();
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 64) throw new BridgeRefusal("bad_input", "limit must be 1..64");
    const items: Record<string, unknown>[] = [];
    const positions = { steer: 0, follow_up: 0, prompt: 0 };
    let bytes = 0, more = false;
    for (const item of this.pending) {
      const entry = { item: item.item, origin: item.origin, mode: item.mode, state: item.state,
        position: ++positions[item.mode], bytes: item.text_bytes, text_sha256: item.text_sha256,
        actions: item.origin === (local ? "pane" : "corral-inbox") ? (local ? ["withdraw", "edit"] : ["withdraw"]) : [],
        ...(item.idempotency_key ? { idempotency_key: item.idempotency_key } : {}),
        ...(local && item.origin === "pane" ? { text: utf8Prefix(item.text, INPUT_MAX_BYTES), truncated: item.text_bytes > INPUT_MAX_BYTES } : {}) };
      bytes += Buffer.byteLength(JSON.stringify(entry));
      if (items.length === limit || bytes > SNAPSHOT_BYTES) { more = true; break; }
      items.push(entry);
    }
    return { available: true, generation: this.options.generation, items, more };
  }

  withdraw(key: string): QueueOutcome {
    this.check();
    return this.mutate(key, `pane-g${this.options.generation}-${++this.operationCounter}`, "pane", "withdraw");
  }
  bridgeWithdraw(key: string, op_key: string): QueueOutcome {
    this.check();
    return this.mutate(key, op_key, "corral", "withdraw");
  }
  operation(op_key: string): QueueOutcome | undefined {
    this.check();
    const operation = this.operations.get(op_key);
    if (!operation?.result) return undefined;
    return operation.replacement
      ? { ...operation.result, replacement: { ...operation.replacement.disposition, item: operation.replacement.item } }
      : { ...operation.result };
  }

  private target(key: string, actor: "pane" | "corral"): Item | undefined {
    if (key.startsWith("q-g") && !key.startsWith(`q-g${this.options.generation}-${this.binding?.bind.slice(0, 12)}-`)) {
      throw new BridgeRefusal("stale_item", "item is not from this host binding");
    }
    const item = this.items.get(key) ?? (actor === "corral" ? this.keys.get(key) : undefined);
    if (item && item.origin !== (actor === "pane" ? "pane" : "corral-inbox")) {
      throw new BridgeRefusal("foreign_origin", item.origin === "corral-inbox" ? "use corral inbox withdraw for this input" : "input belongs to another origin");
    }
    return item;
  }

  private mutate(key: string, op_key: string, actor: "pane" | "corral", reason: string, replacement?: string): QueueOutcome {
    this.check();
    const digest = sha256Hex(canonicalJson({ key, actor, reason, generation: this.options.generation, ...this.binding,
      ...(replacement !== undefined ? { replacement_sha256: sha256Hex(replacement) } : {}) }));
    const prior = this.operations.get(op_key);
    if (prior) {
      if (prior.digest !== digest) throw new BridgeRefusal("op_conflict", "operation key already names another intent");
      if (!prior.result) throw new BridgeRefusal("operation_uncertain", `read operation ${op_key}; do not retry with a fresh key`);
      return { ...prior.result, replayed: true };
    }
    if (this.operations.size >= QUEUE_OPERATIONS_MAX) throw new BridgeRefusal("op_limit", "generation operation limit reached; replays still work");
    const item = this.target(key, actor);
    const operation: Operation = { digest };
    this.operations.set(op_key, operation);
    try {
      let outcome: QueueOutcome["outcome"] = "unknown_item";
      if (item?.state === "started") outcome = "already_started";
      else if (item?.state === "withdrawn") outcome = "already_withdrawn";
      else if (item?.state === "queued" && item.message) {
        this.removal = { op_key, actor, reason };
        try {
          if (!this.options.session().removeQueuedMessage(item.message, actor)) {
            throw new BridgeRefusal("queue_unknown", "queued object missing; withdrawal is uncertain");
          }
          outcome = "withdrawn";
        } finally { this.removal = undefined; }
      }
      const result: QueueOutcome = { op_key, outcome, replayed: false, ...(item ? { item: item.item } : {}),
        ...(item?.turn ? { turn: item.turn } : {}), ...(item?.first_op_key ? { first_op_key: item.first_op_key } : {}) };
      this.append(QUEUE_OPERATION_ENTRY, { version: 1, ...this.binding, generation: this.options.generation,
        intent_sha256: digest, target: key, actor, reason, ...result,
        ...(replacement !== undefined ? { replacement_sha256: sha256Hex(replacement) } : {}) });
      operation.result = result;
      return result;
    } catch (error) {
      throw new BridgeRefusal(error instanceof BridgeRefusal ? error.code : "operation_uncertain",
        `operation ${op_key}: ${error instanceof Error ? error.message : String(error)}; reconcile this key, not a new mutation`);
    }
  }

  /** Withdraw once, then submit a new tail item through native handlers (not the old handler). */
  async edit(key: string, text: string): Promise<QueueOutcome> {
    this.check();
    if (!text.trim() || Buffer.byteLength(text) > INPUT_MAX_BYTES) throw new BridgeRefusal("bad_input", "replacement must be nonblank and at most 8192 bytes");
    const old = this.target(key, "pane");
    const op_key = `pane-g${this.options.generation}-${++this.operationCounter}`;
    const result = this.mutate(key, op_key, "pane", "edit", text);
    if (result.outcome !== "withdrawn" || !old) return result;
    const replacement = this.mint(text, old.mode, "pane", old.images, undefined, old.item);
    const operation = this.operations.get(op_key);
    if (operation) operation.replacement = { item: replacement.item, disposition: { disposition: "pending" } };
    const session = this.options.session();
    const disposition: SubmitResult = await session[old.mode === "steer" ? "steer" : "followUp"](
      text, old.images, { source: "interactive", queueTag: replacement },
    ).then(disposition => ({ disposition }), error => ({ disposition: "rejected", reason: String(error).slice(0, 256) }));
    // The immutable operation receipt proves withdrawal; the replacement's own lifecycle
    // proves admission/rejection and links back through supersedes. No second result authority.
    this.healthy();
    if (operation) operation.replacement = { item: replacement.item, disposition };
    return { ...result, replacement: { ...disposition, item: replacement.item } };
  }
}

function messageText(message: AgentMessage): string {
  if (!("content" in message)) return "";
  return typeof message.content === "string" ? message.content : message.content.filter(part => part.type === "text").map(part => part.text).join("");
}
