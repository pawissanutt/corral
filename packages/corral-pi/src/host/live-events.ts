// Ephemeral chat projection only: never writes the Pi journal or the lifecycle event ring.
import type { AgentSessionEvent } from "@earendil-works/pi-coding-agent";
import { EventLog, EVENT_MAX_BYTES, type EventPage } from "../bridge/events.ts";
import type { FleetSnapshot } from "../build/types.ts";
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

/** App-only replacement projection: never includes child prompts, paths or reports. */
export interface SessionFleet {
  children: { id: string; kind: string; title: string; state: string; model: string;
    started_at?: number; ended_at?: number; activity?: string; tokens: number; cost_usd?: number;
    verdict?: "approve" | "changes"; question?: boolean }[];
  messages: { seq: number; at: number; from: string; to: string; text: string; delivery: string }[];
  messages_total: number;
  max_parallel: number;
}

export class LiveEvents {
  readonly events: EventLog;
  private message = 0;
  private textOffset = 0;
  private thinkingOffset = 0;
  private error: string | undefined;
  private readonly toolCompletions: Record<string, unknown>[] = [];
  private awaitingMessagePersistence = false;
  private fleet: SessionFleet | undefined;
  private fleetJson = "";
  private fleetSeq = 0;

  constructor(capacity = 512) {
    this.events = new EventLog(capacity);
  }

  rewound(turn: number): void {
    this.events.invalidate("live_rewind", { turn });
  }

  /** Initially empty fleets stay silent; an empty replacement after activity is a sequenced clear. */
  observeFleet(snapshot: FleetSnapshot | undefined): void {
    if (!snapshot || !this.fleet && !snapshot.children.length && !snapshot.mail.length) return;
    const fleet: SessionFleet = {
      children: snapshot.children.map(child => ({
        id: child.id, kind: child.kind, title: child.title.slice(0, 256), state: child.state,
        model: child.model.slice(0, 128),
        ...(child.startedAt === undefined ? {} : { started_at: child.startedAt }),
        ...(child.endedAt === undefined ? {} : { ended_at: child.endedAt }),
        // Runtime activity includes tool arguments (often paths); expose only the tool name.
        ...(child.activity ? { activity: child.activity.split(/\s/, 1)[0]!.slice(0, 64) } : {}),
        tokens: child.usage.input + child.usage.output,
        ...(child.usage.cost === undefined ? {} : { cost_usd: child.usage.cost }),
        ...(child.verdict ? { verdict: child.verdict.verdict } : {}),
        ...(child.question ? { question: true } : {}),
      })),
      messages: snapshot.mail.slice(-100).map(message => ({
        seq: message.seq, at: message.at, from: message.from.slice(0, 128), to: message.to.slice(0, 128),
        text: Array.from(message.text).slice(0, 2048).join(""), delivery: message.delivery,
      })),
      messages_total: snapshot.mail.at(-1)?.seq ?? 0, max_parallel: snapshot.maxParallel,
    };
    const json = JSON.stringify(fleet);
    if (json === this.fleetJson) return;
    this.fleet = fleet;
    this.fleetJson = json;
    this.fleetSeq = this.events.emit("live_fleet").seq;
  }

  since(seq: number, maxBytes: number): EventPage & { fleet?: SessionFleet } {
    const initial = this.events.since(seq, maxBytes);
    if (!this.fleet || seq >= this.fleetSeq && !initial.gap) return initial;
    const fleet = { ...this.fleet, children: [...this.fleet.children], messages: [...this.fleet.messages] };
    // Leave room for at least one intact event and JSON field punctuation.
    const first = initial.events[0];
    const required = first?.kind === "owner_prompt" ? Buffer.byteLength(JSON.stringify(first)) : EVENT_MAX_BYTES;
    const budget = Math.max(0, maxBytes - required - 256);
    let size = Buffer.byteLength(JSON.stringify(fleet));
    while (size > budget && (fleet.messages.length || fleet.children.length)) {
      if (fleet.messages.length) fleet.messages.shift();
      else fleet.children.shift();
      size = Buffer.byteLength(JSON.stringify(fleet));
    }
    return { ...this.events.since(seq, maxBytes - size - 16), fleet };
  }

  /** Called after HostSession updates its turn sequence for this same upstream event. */
  observe(event: AgentSessionEvent, turn: number): void {
    switch (event.type) {
      case "agent_start":
        this.message = 0;
        this.textOffset = 0;
        this.thinkingOffset = 0;
        this.error = undefined;
        this.toolCompletions.length = 0;
        this.awaitingMessagePersistence = false;
        this.events.emit("live_turn", { turn, state: "started" });
        break;
      case "agent_settled":
        // Settlement also runs after a failed message append; that is not a replay barrier.
        if (this.awaitingMessagePersistence) break;
        this.events.emit("live_turn", { turn, state: "ended", aborted: event.aborted,
          ...(this.error ? { error: this.error } : {}) });
        break;
      case "message_start":
        if (event.message.role !== "assistant") break;
        this.message++;
        this.textOffset = 0;
        this.thinkingOffset = 0;
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
        if (event.message.role === "assistant" && event.message.stopReason === "error") {
          this.error = utf8Prefix(event.message.errorMessage ?? "provider_error", 512);
        }
        break;
      case "tool_execution_start":
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
        this.toolCompletions.push({ turn, call: event.toolCallId, tool: event.toolName,
          state: "ended", failed: event.isError,
          ...(event.durationMs === undefined ? {} : { elapsed_ms: event.durationMs }) });
        break;
      case "turn_end":
        // Pi awaits every message_end handler (including its synchronous append) before
        // turn_end; tool_execution_end and message_end public listeners are both too early.
        this.awaitingMessagePersistence = false;
        for (const completion of this.toolCompletions) this.events.emit("live_tool", completion);
        this.toolCompletions.length = 0;
        break;
      case "queue_update":
        this.events.emit("live_queue", { steer: event.steering.length, follow_up: event.followUp.length });
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
  }
}
