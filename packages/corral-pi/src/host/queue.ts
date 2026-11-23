// Pi owns queue mutations; this is a live view and a bounded index of just-started inputs.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AgentSession } from "@earendil-works/pi-coding-agent";
import type { QueuedItem } from "./queue-control.ts";
import type { BusyPolicy, SessionInput } from "../bridge/protocol.ts";
import { sha256Hex } from "../tools/corral.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";
import { transcriptInput, type LiveInput, type TranscriptInput } from "./remote/input.ts";
import type { HeldInput } from "./session-record.ts";

const SNAPSHOT_BYTES = 48 * 1024 - 256;
const RECENT_STARTS = 64;
type Origin = TranscriptInput["origin"] | "extension";
type Observer = NonNullable<Parameters<AgentSession["setQueueObserver"]>[0]>;
interface Input {
  key: string;
  text: string;
  origin: Origin;
  mode: BusyPolicy;
  readonly socketId?: string;
  readonly facts: TranscriptInput;
  readonly transcriptId?: string;
  /** Set only by the host, for the one input it admits as an approved plan's kickoff: the review card's id. */
  readonly planKickoff?: string;
  /**
   * The plan review card this input answers, as approval or feedback, and the last entry of the model's context
   * when the answer was given: it starts only while that review is open and nothing was said since.
   */
  readonly planAnswer?: { readonly card: string; readonly context?: string };
}
/** What the transcript records of an input, and who it came from: `owner` and `pane` are the owner's own words. */
export type InputFacts = Pick<Input, "facts" | "transcriptId" | "origin" | "planKickoff" | "planAnswer">;
interface Queued extends Input { mode: "steer" | "follow_up"; message: AgentMessage }
export type Withdrawal = { outcome: "withdrawn" | "already_started" | "unknown_item"; item?: string; turn?: number; text?: string };
export interface QueuedInput {
  key: string; policy: BusyPolicy; origin: TranscriptInput["origin"]; text: string; bytes: number; to?: string; actions: string[];
}

function queuedInput(key: string, text: string, policy: BusyPolicy, origin: QueuedInput["origin"], to?: string): QueuedInput {
  return { key, policy, origin, text: utf8Prefix(text, 200), bytes: Buffer.byteLength(text),
    ...(to === undefined ? {} : { to }), actions: ["withdraw"] };
}

// The SDK can append but not insert. Only the owner-only tail of each policy can move without rewriting foreign items.
function ownerBoundary(ordered: readonly Queued[]): Record<Queued["mode"], number> {
  const foreign = { steer: -1, follow_up: -1 };
  for (let i = 0; i < ordered.length; i++) {
    const item = ordered[i]!;
    if (item.origin !== "owner") foreign[item.mode] = i;
  }
  return foreign;
}

function heldView(item: HeldInput): QueuedInput {
  return queuedInput(item.queueKey, item.text, item.busyPolicy, item.origin, item.to);
}

export interface HostQueueOptions {
  readonly session: () => AgentSession;
  readonly enabled: () => boolean;
  readonly interactive: () => boolean;
  readonly turn: (nextRun: boolean) => number;
  readonly emit: (kind: string, data: Record<string, unknown>) => void;
  readonly liveInput?: (event: LiveInput) => void;
}

export class HostQueue {
  private readonly queued = new Map<string, Queued>();
  private readonly objects = new WeakMap<AgentMessage, Queued>();
  private readonly started = new Map<string, number>();
  private reordering = false;
  private readonly counts = { prompt: 0, steer: 0, follow_up: 0 };
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
          this.started.delete(item.key);
          const position = ++this.counts[item.mode];
          this.queued.set(item.key, item);
          this.objects.set(entry.message, item);
          this.emit(item, "queued", undefined, item.socketId === undefined ? undefined :
            { input: item.socketId, state: "queued", policy: item.mode, position });
          continue;
        }
        const item = this.objects.get(entry.message);
        if (!item) continue;
        this.queued.delete(item.key);
        this.objects.delete(entry.message);
        this.counts[item.mode]--;
        if (change.action === "drain") this.start(item, change.nextRun);
        else this.emit(item, "withdrawn", undefined,
          item.socketId === undefined ? undefined : { input: item.socketId, state: "withdrawn" });
      }
    },
    restoreInput: tag => {
      const item = this.tag(tag);
      return item?.origin === "pane" ? item.text : undefined;
    },
  };

  private input(text: string, mode: BusyPolicy, origin: Origin): Input {
    return { key: `q-${crypto.randomUUID()}`, text, mode, origin, facts: { origin: origin === "extension" ? "auto" : origin } };
  }

  bridgeInput(input: SessionInput, source: {
    socketId?: string; transcriptId?: string; text?: string; pane?: boolean; queueKey?: string; planKickoff?: string;
    planAnswer?: Input["planAnswer"];
  }): unknown {
    const { socketId, transcriptId, pane } = source;
    const origin = pane ? "pane" : input.origin;
    return { key: source.queueKey ?? socketId ?? `q-${crypto.randomUUID()}`, text: source.text ?? input.text,
      mode: input.busyPolicy, origin, socketId, transcriptId,
      ...(source.planKickoff === undefined ? {} : { planKickoff: source.planKickoff }),
      ...(source.planAnswer === undefined ? {} : { planAnswer: source.planAnswer }),
      facts: transcriptInput(input, !pane && socketId === undefined ? "auto" : origin) } satisfies Input;
  }

  inputFacts(tag: unknown): InputFacts | undefined {
    return this.tag(tag);
  }

  has(key: string): boolean { return this.queued.has(key); }
  count(mode: BusyPolicy): number { return this.counts[mode]; }

  private tag(value: unknown): Input | undefined {
    if (!value || typeof value !== "object" || !("key" in value) || !("text" in value) || !("facts" in value)) return undefined;
    return value as Input;
  }

  private emit(item: Input, event: string, extra?: Record<string, unknown>, live?: LiveInput): void {
    queueMicrotask(() => {
      this.options.emit("queue_item", { item: item.key, mode: item.mode, origin: item.origin, event, ...extra });
      if (live) this.options.liveInput?.(live);
    });
  }

  private start(item: Input, nextRun: boolean): void {
    const turn = this.options.turn(nextRun);
    this.started.set(item.key, turn);
    if (this.started.size > RECENT_STARTS) {
      const first = this.started.keys().next().value;
      if (first) this.started.delete(first);
    }
    this.emit(item, "started", { turn },
      item.socketId === undefined ? undefined : { input: item.socketId, state: "started", turn });
  }

  private ordered(): Queued[] {
    return this.options.session().agent.getQueuedMessages().flatMap(message => this.objects.get(message) ?? []);
  }

  snapshot(limit = 64, held: readonly HeldInput[] = []): Record<string, unknown> {
    const items: Record<string, unknown>[] = [];
    const positions = { prompt: 0, steer: 0, follow_up: 0 };
    const ordered = this.ordered();
    const foreign = ownerBoundary(ordered);
    let bytes = 0;
    for (let i = 0; i < ordered.length + held.length; i++) {
      if (items.length === limit) return { available: true, items, more: true };
      const item = ordered[i], saved = held[i - ordered.length];
      const { key, policy, ...view } = item ? this.view(ordered, i, foreign) : heldView(saved!);
      const row = { ...view, item: key, origin: item?.origin ?? view.origin, mode: policy, state: "queued",
        position: ++positions[policy], text_sha256: sha256Hex(item?.text ?? saved!.text) };
      bytes += Buffer.byteLength(JSON.stringify(row)) + 1;
      if (bytes > SNAPSHOT_BYTES) return { available: true, items, more: true };
      items.push(row);
    }
    return { available: true, items, more: false };
  }

  private view(ordered: readonly Queued[], index: number, foreign: Record<Queued["mode"], number>): QueuedInput {
    const item = ordered[index]!;
    const view = queuedInput(item.key, item.text, item.mode, item.origin === "extension" ? "auto" : item.origin, item.facts.to);
    if (index > foreign[item.mode]) {
      if (index - 1 > foreign[item.mode] && ordered[index - 1]?.mode === item.mode) view.actions.push("move_up");
      if (ordered[index + 1]?.mode === item.mode) view.actions.push("move_down");
    }
    return view;
  }

  brief(held: readonly HeldInput[] = []): { queue: QueuedInput[]; queue_total: number } {
    const ordered = this.ordered();
    const foreign = ownerBoundary(ordered);
    const queue = ordered.slice(0, 12).map((_, i) => this.view(ordered, i, foreign));
    for (let i = 0; i < held.length && queue.length < 12; i++) queue.push(heldView(held[i]!));
    return { queue, queue_total: ordered.length + held.length };
  }

  items(): QueuedItem[] {
    return this.ordered().map(item => ({ key: item.key, text: item.text, policy: item.mode,
      editable: item.origin === "pane", origin: item.origin }));
  }

  get controlEnabled(): boolean { return this.options.interactive() && this.options.enabled(); }

  withdrawInput(key: string, includeText = false): Withdrawal {
    const item = this.queued.get(key);
    if (item && this.options.session().removeQueuedMessage(item.message, "corral")) {
      return { outcome: "withdrawn", item: key, ...(includeText ? { text: item.text } : {}) };
    }
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

  moveOwner(key: string, delta: -1 | 1): boolean {
    return this.moveInput(this.queued.get(key), delta, "owner");
  }

  async move(key: string, delta: -1 | 1): Promise<boolean> {
    return this.moveInput(this.editable(key), delta, "pane");
  }

  /** Reorder exact Pi objects without re-admitting or withdrawing any input. */
  private moveInput(item: Queued | undefined, delta: -1 | 1, origin: "owner" | "pane"): boolean {
    if (item?.origin !== origin || this.reordering) return false;
    const session = this.options.session();
    const group = this.ordered().filter(entry => entry.mode === item.mode);
    const index = group.indexOf(item), next = index + delta;
    if (index < 0 || next < 0 || next >= group.length) return false;
    const suffix = group.slice(Math.min(index, next));
    if (suffix.some(entry => entry.origin !== origin)) return false;
    this.reordering = true;
    try {
      for (const entry of suffix) session.removeQueuedMessage(entry.message, origin === "pane" ? "pane_move" : "owner_move");
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
