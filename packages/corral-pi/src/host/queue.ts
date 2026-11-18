// Pi owns queue mutations; this is a live view and a bounded index of just-started inputs.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AgentSession } from "@earendil-works/pi-coding-agent";
import type { QueuedItem } from "./queue-control.ts";
import type { BusyPolicy, SessionInput } from "../bridge/protocol.ts";
import { sha256Hex } from "../tools/corral.ts";

const SNAPSHOT_BYTES = 48 * 1024;
const RECENT_STARTS = 64;
type Origin = "pane" | "extension" | SessionInput["origin"];
type Observer = NonNullable<Parameters<AgentSession["setQueueObserver"]>[0]>;
interface Input {
  key: string;
  text: string;
  origin: Origin;
  mode: BusyPolicy;
}
interface Queued extends Input { mode: "steer" | "follow_up"; message: AgentMessage }
type Withdrawal = { outcome: "withdrawn" | "already_started" | "unknown_item"; item?: string; turn?: number };
export interface HostQueueOptions {
  readonly session: () => AgentSession;
  readonly enabled: () => boolean;
  readonly interactive: () => boolean;
  readonly turn: (nextRun: boolean) => number;
  readonly emit: (kind: string, data: Record<string, unknown>) => void;
}

export class HostQueue {
  private readonly queued = new Map<string, Queued>();
  private readonly objects = new WeakMap<AgentMessage, Queued>();
  private readonly started = new Map<string, number>();
  private reordering = false;
  constructor(private readonly options: HostQueueOptions) {}

  readonly observer: Observer = {
    input: input => this.input(input.text, input.mode, input.source === "interactive" ? "pane" : "extension"),
    lifecycle: ({ tag, event, reason }) => {
      const item = this.tag(tag);
      if (!item || event === "queued" || this.queued.has(item.key)) return;
      if (event === "started") this.start(item, true);
      else this.emit(item, event, { reason });
    },
    queue: change => {
      if (this.reordering) return;
      for (const entry of change.entries) {
        if (change.action === "enqueue") {
          const input = this.tag(entry.tag) ?? this.input(messageText(entry.message), change.mode, "extension");
          const item = { ...input, mode: change.mode, message: entry.message };
          this.queued.set(item.key, item);
          this.objects.set(entry.message, item);
          this.emit(item, "queued");
          continue;
        }
        const item = this.objects.get(entry.message);
        if (!item) continue;
        this.queued.delete(item.key);
        this.objects.delete(entry.message);
        if (change.action === "drain") this.start(item, change.nextRun);
        else this.emit(item, "withdrawn");
      }
    },
    restoreInput: tag => {
      const item = this.tag(tag);
      return item?.origin === "pane" ? item.text : undefined;
    },
  };

  private input(text: string, mode: BusyPolicy, origin: Origin): Input {
    return { key: `q-${crypto.randomUUID()}`, text, mode, origin };
  }

  bridgeInput(input: SessionInput): unknown { return this.input(input.text, input.busyPolicy, input.origin); }

  private tag(value: unknown): Input | undefined {
    if (!value || typeof value !== "object" || !("key" in value) || !("text" in value)) return undefined;
    return value as Input;
  }

  private emit(item: Input, event: string, extra?: Record<string, unknown>): void {
    queueMicrotask(() => this.options.emit("queue_item", { item: item.key, mode: item.mode, origin: item.origin, event, ...extra }));
  }

  private start(item: Input, nextRun: boolean): void {
    const turn = this.options.turn(nextRun);
    this.started.set(item.key, turn);
    if (this.started.size > RECENT_STARTS) {
      const first = this.started.keys().next().value;
      if (first) this.started.delete(first);
    }
    this.emit(item, "started", { turn });
  }

  private ordered(): Queued[] {
    return this.options.session().agent.getQueuedMessages().flatMap(message => this.objects.get(message) ?? []);
  }

  snapshot(limit = 64): Record<string, unknown> {
    const items: Record<string, unknown>[] = [];
    const positions = { steer: 0, follow_up: 0 };
    let bytes = 0;
    for (const item of this.ordered()) {
      const entry = { item: item.key, origin: item.origin, mode: item.mode, state: "queued",
        position: ++positions[item.mode], bytes: Buffer.byteLength(item.text), text_sha256: sha256Hex(item.text), actions: ["withdraw"] };
      bytes += Buffer.byteLength(JSON.stringify(entry));
      if (items.length === limit || bytes > SNAPSHOT_BYTES) return { available: true, items, more: true };
      items.push(entry);
    }
    return { available: true, items, more: false };
  }

  items(): QueuedItem[] {
    return this.ordered().map(item => ({ key: item.key, text: item.text, policy: item.mode,
      editable: item.origin === "pane", origin: item.origin }));
  }

  get controlEnabled(): boolean { return this.options.interactive() && this.options.enabled(); }

  withdrawInput(key: string): Withdrawal {
    const item = this.queued.get(key);
    if (item && this.options.session().removeQueuedMessage(item.message, "corral")) return { outcome: "withdrawn", item: key };
    const turn = this.started.get(key);
    return turn === undefined ? { outcome: "unknown_item" } : { outcome: "already_started", item: key, turn };
  }

  private editable(key: string): Queued | undefined {
    const item = this.queued.get(key);
    return this.controlEnabled && item?.origin === "pane" ? item : undefined;
  }

  async remove(key: string): Promise<boolean> {
    const item = this.editable(key);
    return !!item && this.options.session().removeQueuedMessage(item.message, "pane");
  }

  async edit(key: string): Promise<string | undefined> {
    const item = this.editable(key);
    return item && await this.remove(key) ? item.text : undefined;
  }

  /** The SDK can remove and append the same object, so move an editable suffix as one batch. */
  async move(key: string, delta: -1 | 1): Promise<boolean> {
    const item = this.editable(key);
    if (!item || this.reordering) return false;
    const session = this.options.session();
    const group = this.ordered().filter(entry => entry.mode === item.mode);
    const index = group.indexOf(item), next = index + delta;
    if (index < 0 || next < 0 || next >= group.length) return false;
    const suffix = group.slice(Math.min(index, next));
    if (suffix.some(entry => entry.origin !== "pane")) return false;
    this.reordering = true;
    try {
      for (const entry of suffix) session.removeQueuedMessage(entry.message, "pane_move");
      const first = suffix[0], second = suffix[1];
      if (first && second) { suffix[0] = second; suffix[1] = first; }
      for (const entry of suffix) session.agent[item.mode === "steer" ? "steer" : "followUp"](entry.message);
    } finally { this.reordering = false; }
    return true;
  }
}

function messageText(message: AgentMessage): string {
  if (!("content" in message)) return "";
  return typeof message.content === "string" ? message.content : message.content.filter(part => part.type === "text").map(part => part.text).join("");
}
