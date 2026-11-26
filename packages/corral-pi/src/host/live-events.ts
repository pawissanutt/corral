// Ephemeral chat projection only: never writes the Pi journal or the lifecycle event ring.
import type { AgentSessionEvent } from "@earendil-works/pi-coding-agent";
import { EventLog, EVENT_MAX_BYTES, type EventPage } from "../bridge/events.ts";
import type { FleetSnapshot } from "../build/types.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { boundFleet, projectFleet, type SessionFleet } from "./remote/fleet.ts";
import type { LiveInput } from "./remote/input.ts";
import { boundState, type Activity, type Counts, type HostNotice, type SessionPart,
  type BoundedSessionState, type SessionState, type StateProjection } from "./remote/state.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";

// JSON escaping can multiply a payload by six; leave room inside the 8 KiB event bound.
const PAYLOAD_BYTES = 1024;
const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

function argumentsText(value: unknown): Record<string, unknown> {
  if (value === undefined) return {};
  try {
    const json = JSON.stringify(value);
    if (json === undefined) return { args_truncated: true };
    const args = utf8Prefix(json, PAYLOAD_BYTES);
    return { args, ...(args !== json ? { args_truncated: true } : {}) };
  } catch {
    // Extension-supplied values need not be serializable; observation must not stop the tool.
    return { args_truncated: true };
  }
}

function outputTail(value: unknown): string | undefined {
  if (!object(value) || !Array.isArray(value.content)) return undefined;
  let tail = "";
  for (let i = value.content.length - 1; i >= 0; i--) {
    const part: unknown = value.content[i];
    if (!object(part) || part.type !== "text" || typeof part.text !== "string") continue;
    const bytes = Buffer.from(part.text.slice(-PAYLOAD_BYTES) + tail, "utf8");
    let start = Math.max(0, bytes.length - PAYLOAD_BYTES);
    while (start < bytes.length && ((bytes[start] ?? 0) & 0xc0) === 0x80) start++;
    tail = bytes.subarray(start).toString("utf8");
    if (start > 0 || Buffer.byteLength(tail) === PAYLOAD_BYTES) break;
  }
  return tail;
}

export type { SessionFleet } from "./remote/fleet.ts";
const STATE_INTERVAL_MS = 250;
const NOTICE_MS = 8_000;
const PAGE_BYTES = 56 * 1024;
// What the host keeps of the message being written for a reader that joins it late.
const OPEN_KEEP_BYTES = 32 * 1024;
const OPEN_TAIL_BYTES = 8 * 1024;
const OPEN_BYTES = 24 * 1024;

/** Where the open turn is, for a reader whose cursor fell out of the ring. */
export interface OpenTurn {
  readonly turn: number;
  /** Ordinal of the turn's latest assistant message; 0 before the first. */
  readonly message: number;
  readonly ended: boolean;
  readonly text_offset: number;
  readonly text: string;
  readonly thinking_offset: number;
  readonly thinking: string;
}

/** The last `limit` bytes of `bytes` from a character boundary, and the bytes before them. */
function tail(bytes: Buffer, limit: number): { text: string; skipped: number } {
  let start = Math.max(0, bytes.length - limit);
  while (start < bytes.length && ((bytes[start] ?? 0) & 0xc0) === 0x80) start++;
  return { text: bytes.subarray(start).toString("utf8"), skipped: start };
}

export class LiveEvents {
  readonly events: EventLog;
  private message = 0;
  private textOffset = 0;
  private thinkingOffset = 0;
  private turn: number | undefined;
  private messageEnded = true;
  private written = { live_text: Buffer.alloc(0), live_thinking: Buffer.alloc(0) };
  private error: string | undefined;
  private readonly toolCompletions: Record<string, unknown>[] = [];
  private awaitingMessagePersistence = false;
  private fleet: SessionFleet | undefined;
  private fleetJson = "";
  private fleetSeq = 0;
  private fleetBytes = 0;
  private childrenJson = "[]";
  childCounts: Counts = { live: 0, waiting: 0, total: 0 };
  activity: Activity | undefined;
  private readState: (() => StateProjection) | undefined;
  private state: BoundedSessionState | undefined;
  private stateSeq = 0;
  private sampledAt = -Infinity;
  private stateTimer: ReturnType<typeof setTimeout> | undefined;
  private noticeTimer: ReturnType<typeof setTimeout> | undefined;
  private noticeId = 0;
  private hostNotices: HostNotice[] = [];
  private readonly parts = new Map<SessionPart, string>();

  constructor(capacity = 512) {
    this.events = new EventLog(capacity);
  }

  rewound(turn: number): void {
    this.events.invalidate("live_rewind", { turn });
    this.stateChanged();
  }

  bindState(read: () => StateProjection): void {
    this.readState = read;
    this.refreshState();
  }

  stateChanged(): void {
    if (!this.readState || this.stateTimer) return;
    const wait = Math.max(0, Math.ceil(this.sampledAt + STATE_INTERVAL_MS - performance.now()));
    // Even when the cooldown elapsed, let the SDK finish the synchronous mutation first.
    this.stateTimer = setTimeout(() => this.refreshState(), wait);
    this.stateTimer.unref();
  }

  private refreshState(): void {
    clearTimeout(this.stateTimer);
    this.stateTimer = undefined;
    if (!this.readState) return;
    this.sampledAt = performance.now();
    const { state, parts } = this.readState();
    for (const part of Object.keys(parts) as SessionPart[]) this.observePart(part, parts[part]);
    const next = boundState(state);
    if (this.state?.json === next.json) return;
    this.state = next;
    this.stateSeq = this.events.emit("live_state").seq;
  }

  observePart(part: SessionPart, value: unknown): void {
    const json = JSON.stringify(value ?? null);
    const previous = this.parts.get(part);
    if (json === previous) return;
    this.parts.set(part, json);
    if (previous !== undefined) this.events.emit("live_part", { part });
  }

  input(event: LiveInput): void {
    this.events.emit("live_input", event);
    if (event.state === "queued" || event.state === "started" || event.state === "withdrawn") this.stateChanged();
  }

  entry(): void {
    this.events.emit("live_entry");
    this.stateChanged();
  }

  note(tone: HostNotice["tone"], text: string, at: number): void {
    this.hostNotices = [...this.hostNotices.slice(-3), { id: `note-${++this.noticeId}`, tone, text: utf8Prefix(text, 240),
      until: new Date(at + NOTICE_MS).toISOString() }];
    this.expireNotices();
    this.stateChanged();
  }

  get notices(): HostNotice[] { return this.hostNotices; }

  private expireNotices(): void {
    clearTimeout(this.noticeTimer);
    this.noticeTimer = undefined;
    const now = Date.now();
    const active = this.hostNotices.filter(notice => Date.parse(notice.until!) > now);
    if (active.length !== this.hostNotices.length) {
      this.hostNotices = active;
      this.stateChanged();
    }
    if (active[0]) {
      this.noticeTimer = setTimeout(() => this.expireNotices(), Date.parse(active[0].until!) - now);
      this.noticeTimer.unref();
    }
  }

  close(): void {
    clearTimeout(this.stateTimer);
    clearTimeout(this.noticeTimer);
    this.readState = undefined;
  }

  /** Initially empty fleets stay silent; an empty replacement after activity is a sequenced clear. */
  observeFleet(snapshot: FleetSnapshot | undefined, helpers?: readonly HelperInspection[]): void {
    if (!this.fleet && !snapshot?.children.length && !snapshot?.mail.length && !helpers?.length) return;
    const projection = projectFleet(snapshot, helpers);
    this.childCounts = projection.counts;
    const fleet = boundFleet(projection.fleet);
    const json = JSON.stringify(fleet);
    if (json !== this.fleetJson) {
      this.fleet = fleet;
      this.fleetJson = json;
      this.fleetBytes = Buffer.byteLength(json);
      this.fleetSeq = this.events.emit("live_fleet").seq;
    }
    const children = JSON.stringify(projection.fleet.children);
    if (children !== this.childrenJson) {
      this.childrenJson = children;
      this.events.emit("live_part", { part: "children" });
    }
  }

  /** The open turn as of the latest event, in at most `room` bytes: a reader that joins
   * cannot rebuild it from the ring. Without its tails it is a few hundred bytes. */
  private open(room: number): OpenTurn | undefined {
    if (this.turn === undefined) return undefined;
    const bound = Math.min(OPEN_BYTES, room);
    for (let limit = OPEN_TAIL_BYTES; ; limit >>= 1) {
      const text = tail(this.written.live_text, this.messageEnded ? 0 : limit);
      const thinking = tail(this.written.live_thinking, this.messageEnded ? 0 : limit);
      const open = { turn: this.turn, message: this.message, ended: this.messageEnded,
        text_offset: this.textOffset - this.written.live_text.length + text.skipped, text: text.text,
        thinking_offset: this.thinkingOffset - this.written.live_thinking.length + thinking.skipped,
        thinking: thinking.text };
      if (limit === 0 || Buffer.byteLength(JSON.stringify(open)) <= bound) return open;
    }
  }

  since(seq: number, maxBytes: number): EventPage & { fleet?: SessionFleet; state?: SessionState; open?: OpenTurn } {
    if (this.stateTimer && performance.now() >= this.sampledAt + STATE_INTERVAL_MS) this.refreshState();
    const initial = this.events.since(seq, 0, 1);
    const fleet = initial.gap || seq < this.fleetSeq ? this.fleet : undefined;
    const state = initial.gap || seq < this.stateSeq ? this.state : undefined;
    const room = Math.min(maxBytes, PAGE_BYTES) - (fleet ? this.fleetBytes : 0) - (state?.bytes ?? 0) - 256;
    // A reader that joins (its first read, or a gap) is told where the turn is. The fleet and
    // the state come whole, so the turn's tails give way to them and to one event.
    const open = initial.gap || seq === 0 ? this.open(room - EVENT_MAX_BYTES) : undefined;
    const budget = room - (open ? Buffer.byteLength(JSON.stringify(open)) : 0);
    const page = this.events.since(seq, budget, 32);
    // Older owner prompts may carry a full question larger than a live event. Preserve its identity
    // and status so a reader can reread the canonical card; keep both replacements in the page.
    const events = page.events.map(event => {
      if (event.kind !== "owner_prompt" || Buffer.byteLength(JSON.stringify(event)) <= EVENT_MAX_BYTES) return event;
      const { prompts: _prompts, ...brief } = event;
      return { ...brief, omitted: "event_too_large" };
    });
    return { ...page, events, ...(fleet ? { fleet } : {}), ...(state ? { state: state.state } : {}),
      ...(open ? { open } : {}) };
  }

  /** Called after HostSession updates its turn sequence for this same upstream event. */
  observe(event: AgentSessionEvent, turn: number): void {
    switch (event.type) {
      case "agent_start":
        this.message = 0;
        this.textOffset = 0;
        this.thinkingOffset = 0;
        this.turn = turn;
        this.messageEnded = true;
        this.written = { live_text: Buffer.alloc(0), live_thinking: Buffer.alloc(0) };
        this.error = undefined;
        this.toolCompletions.length = 0;
        this.awaitingMessagePersistence = false;
        this.activity = { phase: "working", since: new Date().toISOString() };
        this.stateChanged();
        this.events.emit("live_turn", { turn, state: "started" });
        break;
      case "agent_settled":
        this.activity = undefined;
        this.stateChanged();
        // Settlement also runs after a failed message append; that is not a replay barrier.
        if (this.awaitingMessagePersistence) break;
        this.turn = undefined;
        this.events.emit("live_turn", { turn, state: "ended", aborted: event.aborted,
          ...(this.error ? { error: this.error } : {}) });
        break;
      case "message_start":
        if (event.message.role !== "assistant") break;
        this.message++;
        this.textOffset = 0;
        this.thinkingOffset = 0;
        this.messageEnded = false;
        this.written = { live_text: Buffer.alloc(0), live_thinking: Buffer.alloc(0) };
        this.error = undefined;
        break;
      case "message_update": {
        const update = event.assistantMessageEvent;
        if (update.type === "text_delta") this.delta("live_text", turn, update.delta);
        else if (update.type === "thinking_delta") this.delta("live_thinking", turn, update.delta);
        break;
      }
      case "message_end":
        if (["user", "assistant", "toolResult", "system"].includes(event.message.role)) {
          this.awaitingMessagePersistence = true;
        }
        if (event.message.role === "assistant") {
          this.messageEnded = true;
          this.written = { live_text: Buffer.alloc(0), live_thinking: Buffer.alloc(0) };
        }
        if (event.message.role === "assistant" && event.message.stopReason === "error") {
          this.error = utf8Prefix(event.message.errorMessage ?? "provider_error", 512);
        }
        break;
      case "tool_execution_start":
        this.activity = { phase: utf8Prefix(event.toolName, 96), since: new Date().toISOString() };
        this.stateChanged();
        this.events.emit("live_tool", { turn, call: event.toolCallId, tool: event.toolName,
          state: "started", ...argumentsText(event.args) });
        break;
      case "tool_execution_update": {
        const partial = outputTail(event.partialResult);
        this.events.emit("live_tool", { turn, call: event.toolCallId, tool: event.toolName,
          state: "updated", ...(partial === undefined ? {} : { partial }) });
        break;
      }
      case "tool_execution_end":
        this.activity = { phase: "working", since: new Date().toISOString() };
        this.stateChanged();
        this.toolCompletions.push({ turn, call: event.toolCallId, tool: event.toolName,
          state: "ended", failed: event.isError,
          ...(event.durationMs === undefined ? {} : { elapsed_ms: event.durationMs }) });
        break;
      case "turn_end":
        // Pi awaits every message_end handler (including its synchronous append) before
        // turn_end; tool_execution_end and message_end public listeners are both too early.
        this.awaitingMessagePersistence = false;
        for (const completion of this.toolCompletions) this.events.emit("live_tool", completion);
        this.stateChanged();
        this.toolCompletions.length = 0;
        break;
      case "queue_update":
        this.events.emit("live_queue", { steer: event.steering.length, follow_up: event.followUp.length });
        this.stateChanged();
        break;
      case "compaction_start":
        this.activity = { phase: "compacting", since: new Date().toISOString() };
        this.stateChanged();
        break;
      case "compaction_end":
        this.activity = this.activity?.phase === "compacting" ? undefined : this.activity;
        this.stateChanged();
        break;
    }
  }

  private delta(kind: "live_text" | "live_thinking", turn: number, text: string): void {
    const bytes = Buffer.from(text, "utf8");
    for (let start = 0; start < bytes.length;) {
      let end = Math.min(start + PAYLOAD_BYTES, bytes.length);
      while (end < bytes.length && ((bytes[end] ?? 0) & 0xc0) === 0x80) end--;
      const offset = kind === "live_text" ? this.textOffset : this.thinkingOffset;
      this.events.emit(kind, { turn, message: this.message, offset, text: bytes.subarray(start, end).toString("utf8") });
      if (kind === "live_text") this.textOffset += end - start;
      else this.thinkingOffset += end - start;
      start = end;
    }
    // The bytes as emitted, so a tail's offset is the one its chunks carried.
    const kept = Buffer.concat([this.written[kind], bytes]);
    this.written[kind] = kept.length > 2 * OPEN_KEEP_BYTES
      ? Buffer.from(kept.subarray(kept.length - OPEN_KEEP_BYTES)) : kept;
  }
}
