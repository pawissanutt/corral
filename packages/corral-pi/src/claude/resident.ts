// One long-lived Claude Code process per Pi session, ported from senpi's anthropic-subscription
// session-registry.js and session-sync.js (MIT). The first turn seeds the process with the flattened
// replay; later turns push only the messages Pi added since, through the SDK's streaming input.
// Anything the registry cannot prove (changed spawn options, rewritten history, a failed or aborted
// turn, an idle process) closes the process, and the next turn seeds a fresh one.
import { createHash } from "node:crypto";

import type { Query, SDKMessage, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { ImageContent, Message, TextContent } from "@earendil-works/pi-ai";

import { appendContent, type PromptBlock } from "./prompt.ts";
import { mapPiToolNameToSdk } from "./tools.ts";

// Offline proof (pi-native-claude-resident-proof.md): ~277 MiB RSS per idle process.
export const RESIDENT_IDLE_MS = 10 * 60_000;
export const RESIDENT_MAX_ENTRIES = 4;

/** The streaming input a resident query reads; it stays open between turns. */
export class InputQueue implements AsyncIterator<SDKUserMessage> {
  private closed = false;
  private readonly pending: SDKUserMessage[] = [];
  private readonly readers: ((result: IteratorResult<SDKUserMessage>) => void)[] = [];

  [Symbol.asyncIterator](): AsyncIterator<SDKUserMessage> {
    return this;
  }

  next(): Promise<IteratorResult<SDKUserMessage>> {
    const message = this.pending.shift();
    if (message) return Promise.resolve({ value: message, done: false });
    if (this.closed) return Promise.resolve({ value: undefined, done: true });
    const { promise, resolve } = Promise.withResolvers<IteratorResult<SDKUserMessage>>();
    this.readers.push(resolve);
    return promise;
  }

  push(message: SDKUserMessage): void {
    if (this.closed) throw new Error("claude_resident_closed: the resident session input is closed");
    const reader = this.readers.shift();
    if (reader) reader({ value: message, done: false });
    else this.pending.push(message);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.pending.length = 0;
    for (const reader of this.readers.splice(0)) reader({ value: undefined, done: true });
  }
}

type Content = string | readonly (TextContent | ImageContent)[];

function contentless(content: Content): boolean {
  return content.length === 0;
}

/** What a resident process has seen: Pi's conversation without system state or content-less user turns. */
export function residentTranscript(messages: readonly Message[]): Message[] {
  return messages.filter((message) => message.role !== "system" &&
    !(message.role === "user" && contentless(message.content)));
}

/** Hash canonical content as well as route identity: any earlier rewrite invalidates continuity. */
export function messageHash(message: Message): string {
  const projected = message.role === "assistant"
    ? { role: message.role, provider: message.provider, model: message.model, content: message.content }
    : message.role === "toolResult"
      ? { role: message.role, id: message.toolCallId, name: message.toolName, content: message.content }
      : { role: message.role, content: "content" in message ? message.content : null };
  return createHash("sha256").update(JSON.stringify(projected)).digest("hex");
}

function placeholder(content: Content): string {
  return typeof content !== "string" && content.some((block) => block.type === "image")
    ? "(see attached image)" : "(no output)";
}

function merge(parts: readonly PromptBlock[]): PromptBlock[] {
  const blocks: PromptBlock[] = [];
  for (const part of parts) {
    const last = blocks.at(-1);
    if (part.type === "text" && last?.type === "text") blocks[blocks.length - 1] = { type: "text", text: last.text + part.text };
    else blocks.push(part);
  }
  return blocks.filter((block) => block.type !== "text" || block.text.length > 0);
}

/**
 * The next user message of a resident process: only the tool results and user messages Pi added since
 * its last turn, with tool names as that process declared them. Changed host question state leads.
 */
export function deltaBlocks(
  messages: readonly Message[], customToolNameToSdk: ReadonlyMap<string, string>, questionState?: string,
): PromptBlock[] {
  const parts: PromptBlock[] = [];
  const text = (value: string) => parts.push({ type: "text", text: value });
  if (questionState !== undefined) text(`<host_question_state>\n${questionState}\n</host_question_state>\n\n`);
  for (const [index, message] of messages.entries()) {
    if (index > 0) text("\n\n");
    if (message.role === "toolResult") {
      text(`Tool result (${mapPiToolNameToSdk(message.toolName, customToolNameToSdk)}, id=${message.toolCallId}):\n`);
      if (!appendContent(parts, message.content)) text(placeholder(message.content));
    } else if (message.role === "user") {
      if (!appendContent(parts, message.content)) text(placeholder(message.content));
    }
  }
  return merge(parts);
}

export interface ResidentEntry {
  readonly sessionId: string;
  readonly scope: string;
  readonly query: Query;
  readonly input: InputQueue;
  /** Settles when the registry closes this process. */
  readonly closed: Promise<void>;
  /** Hashes of the transcript this process has seen, through its last reply. */
  hashes: readonly string[];
  questionState?: string;
  /** Tool-use replies wait for Pi's results and cannot be evicted to admit another session. */
  pinned: boolean;
  /** SDK notices describe the process, including notices read between two turns. */
  rateLimit?: unknown;
  busy: boolean;
  lastUsedAt: number;
}

interface Slot {
  readonly entry: ResidentEntry;
  readonly markClosed: () => void;
  cancelIdle?: () => void;
}

export type ResidentPlan =
  | { readonly kind: "delta"; readonly entry: ResidentEntry; readonly messages: readonly Message[] }
  | { readonly kind: "seed"; readonly entry: ResidentEntry; readonly reason: "cold" | "changed" | "diverged" | "idle" }
  | { readonly kind: "one-shot"; readonly reason: "busy" | "capacity" };

/** Schedule idle expiry and return its cancellation; tests supply a deterministic scheduler. */
export type ResidentScheduler = (expire: () => void, delayMs: number) => () => void;

const scheduleResidentIdle: ResidentScheduler = (expire, delayMs) => {
  const timer = setTimeout(expire, delayMs);
  timer.unref?.();
  return () => clearTimeout(timer);
};

export class ResidentSessions {
  private readonly slots = new Map<string, Slot>();
  /** Timer expiry must remain distinguishable from a cold start after the slot is gone. */
  private readonly expired = new Set<string>();
  private exitHooked = false;
  private readonly onExit = () => this.closeAll();

  constructor(
    private readonly now: () => number = Date.now,
    private readonly schedule: ResidentScheduler = scheduleResidentIdle,
  ) {}

  get size(): number {
    return this.slots.size;
  }

  /** Plans and opens synchronously: a cold seed owns a busy slot before another call can plan. */
  plan(
    sessionId: string, scope: string, transcript: readonly Message[], hashes: readonly string[],
    start: (input: InputQueue) => Query,
  ): ResidentPlan {
    const slot = this.slots.get(sessionId);
    if (slot?.entry.busy) return { kind: "one-shot", reason: "busy" };
    let reason: "cold" | "changed" | "diverged" | "idle" = this.expired.has(sessionId) ? "idle" : "cold";
    if (slot) {
      const { entry } = slot;
      const prefix = entry.hashes.length < hashes.length && entry.hashes.every((hash, index) => hash === hashes[index]);
      const added = prefix ? transcript.slice(entry.hashes.length) : [];
      if (entry.scope !== scope) reason = "changed";
      else if (this.now() - entry.lastUsedAt >= RESIDENT_IDLE_MS) reason = "idle";
      else if (!prefix || added.some(message => message.role !== "user" && message.role !== "toolResult")) reason = "diverged";
      else {
        slot.cancelIdle?.();
        slot.cancelIdle = undefined;
        entry.busy = true;
        entry.lastUsedAt = this.now();
        return { kind: "delta", entry, messages: added };
      }
      this.close(entry);
    }
    if (this.slots.size >= RESIDENT_MAX_ENTRIES && !this.evictIdle()) return { kind: "one-shot", reason: "capacity" };
    this.expired.delete(sessionId);
    return { kind: "seed", reason, entry: this.open(sessionId, scope, start) };
  }

  /** Spawns the session's process; the returned entry is busy until `release` or `close`. */
  private open(sessionId: string, scope: string, start: (input: InputQueue) => Query): ResidentEntry {
    const input = new InputQueue();
    const { promise: closed, resolve: markClosed } = Promise.withResolvers<void>();
    const entry: ResidentEntry = { sessionId, scope, query: start(input), input, closed, hashes: [], busy: true, pinned: false,
      lastUsedAt: this.now() };
    this.slots.set(sessionId, { entry, markClosed });
    if (!this.exitHooked) {
      this.exitHooked = true;
      process.once("exit", this.onExit);
    }
    return entry;
  }

  /** Only a completed, bound reply can advance the process's derived transcript. */
  release(entry: ResidentEntry, hashes: readonly string[], waitingForTools: boolean): void {
    const slot = this.slots.get(entry.sessionId);
    if (slot?.entry !== entry) return;
    entry.hashes = hashes;
    entry.pinned = waitingForTools;
    entry.busy = false;
    entry.lastUsedAt = this.now();
    slot.cancelIdle = this.schedule(() => {
      if (this.slots.get(entry.sessionId) !== slot || entry.busy) return;
      this.close(entry);
      this.expired.add(entry.sessionId);
      if (this.expired.size > RESIDENT_MAX_ENTRIES) {
        const oldest = this.expired.values().next().value;
        if (oldest !== undefined) this.expired.delete(oldest);
      }
    }, RESIDENT_IDLE_MS);
  }

  close(entry: ResidentEntry): void {
    const slot = this.slots.get(entry.sessionId);
    // A replaced entry was closed when it was replaced.
    if (slot?.entry !== entry) return;
    this.slots.delete(entry.sessionId);
    slot.cancelIdle?.();
    entry.input.close();
    try {
      entry.query.close();
    } finally {
      slot.markClosed();
    }
  }

  closeSession(sessionId: string): void {
    const slot = this.slots.get(sessionId);
    if (slot) this.close(slot.entry);
    this.expired.delete(sessionId);
  }

  closeAll(): void {
    for (const slot of [...this.slots.values()]) this.close(slot.entry);
    process.removeListener("exit", this.onExit);
    this.exitHooked = false;
    this.expired.clear();
  }

  private evictIdle(): boolean {
    const oldest = [...this.slots.values()].filter((slot) => !slot.entry.busy && !slot.entry.pinned)
      .sort((left, right) => left.entry.lastUsedAt - right.entry.lastUsedAt)[0];
    if (!oldest) return false;
    this.close(oldest.entry);
    return true;
  }
}

/** Read manually: breaking a for-await over Query itself would terminate the resident process. */
export async function* residentTurn(entry: ResidentEntry, uuid: string): AsyncGenerator<SDKMessage> {
  const closed = entry.closed.then((): IteratorResult<SDKMessage> => ({ done: true, value: undefined }));
  for (;;) {
    const next = await Promise.race([entry.query.next(), closed]);
    if (next.done) return;
    if (next.value.type === "rate_limit_event") entry.rateLimit = next.value.rate_limit_info;
    if (next.value.type === "result" && next.value.user_message_uuid !== uuid) {
      throw new Error("claude_resident_result_mismatch: result did not echo the pushed message id");
    }
    yield next.value;
    if (next.value.type === "result") return;
  }
}

/** Shared across provider re-registration, including child hosts. */
export const residentSessions = new ResidentSessions();

export function closeResidentSession(sessionId: string): void {
  residentSessions.closeSession(sessionId);
}

export function closeResidentSessions(): void {
  residentSessions.closeAll();
}
