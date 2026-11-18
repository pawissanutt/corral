import { createHash } from "node:crypto";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type { AuxiliaryKind, Outcome } from "./runtime.ts";

export type HelperEvent =
  | { readonly type: "error" | "inspection_error"; readonly text: string }
  | { readonly type: "assistant_start" }
  | { readonly type: "assistant_delta" | "assistant_end"; readonly text: string; readonly stopReason?: AssistantMessage["stopReason"]; readonly hasToolCalls?: boolean; readonly errorMessage?: string; readonly textBlocks?: readonly string[] }
  | { readonly type: "tool"; readonly id: string; readonly name: string; readonly phase: "requested" | "running" | "completed" | "error"; readonly arguments?: string; readonly argsComplete?: boolean; readonly text?: string; readonly partial?: boolean };
export interface HelperMessage {
  readonly id: string;
  readonly kind: "assistant" | "tool";
  readonly text: string;
  readonly stopReason?: AssistantMessage["stopReason"];
  readonly hasToolCalls?: boolean;
  readonly errorMessage?: string;
  readonly textBlocks?: readonly string[];
  readonly tool?: string;
  readonly arguments?: string;
  /** False while the bounded argument preview is still being written. */
  readonly argsComplete?: boolean;
  /** True when text is a live result tail, rather than the final result. */
  readonly partial?: boolean;
  readonly phase?: "requested" | "running" | "completed" | "error";
  /** When the projection first saw this message (epoch ms); absent in projections that predate it. */
  readonly at?: number;
}
export interface HelperInspection {
  readonly request: string; readonly kind: AuxiliaryKind; readonly model: string;
  readonly serviceTier?: "priority";
  readonly prompt: string; readonly phase: "preparing" | "queued" | "running" | Outcome;
  readonly createdAt: number; readonly startedAt?: number; readonly endedAt?: number;
  readonly messages: readonly HelperMessage[]; readonly result?: string; readonly error?: string;
  /** Latest visible activity tail, retained independently of clipped transcript heads. */
  readonly latestActivity?: string;
  readonly truncated: boolean;
  /** Messages dropped from the front of `messages` by the retention limit; absent when none were. */
  readonly dropped?: number;
  readonly inspectionError?: string;
}
type MutableMessage = { -readonly [K in keyof HelperMessage]: HelperMessage[K] };
type Retained = Omit<{ -readonly [K in keyof HelperInspection]: HelperInspection[K] }, "messages"> & {
  messages: MutableMessage[]; sequence: number; size: number; assistant?: string; assistantLine?: string;
};
export const HELPER_HISTORY_LIMIT = 32;
export const HELPER_TEXT_LIMIT = 4096;
const TRANSCRIPT_LIMIT = 32768;
function messageSize(message: HelperMessage): number {
  return Math.max(message.text.length, message.textBlocks?.reduce((sum, text) => sum + text.length, 0) ?? 0)
    + (message.errorMessage?.length ?? 0) + (message.arguments?.length ?? 0) + (message.tool?.length ?? 0);
}
function diagnostic(error: unknown): string {
  try { return (error instanceof Error ? error.message : String(error)).slice(0, HELPER_TEXT_LIMIT); }
  catch { return "unavailable diagnostic"; }
}
function activityLine(text: string, max = 200): string {
  const visible = text.trimEnd();
  return visible.slice(visible.lastIndexOf("\n") + 1).replace(/[\r\u2028\u2029]/g, " ")
    .slice(-max).replace(/^[\uDC00-\uDFFF]/, "");
}

/** In-memory, parent-local UI projection. Never opens session files or changes execution. */
export class AuxiliaryHistory {
  private readonly entries = new Map<string, Retained>();
  private notificationError: string | undefined;
  private readonly listeners = new Set<() => void>();
  private disposed = false;
  constructor(private readonly changed: () => void = () => {}, private readonly now: () => number = Date.now) {}
  subscribe(changed: () => void): () => void {
    if (!this.disposed) this.listeners.add(changed);
    return () => { this.listeners.delete(changed); };
  }
  dispose(): void { this.disposed = true; this.listeners.clear(); }
  private notify(): void {
    if (this.disposed) return;
    this.notificationError = undefined;
    this.notifyListener(this.changed);
    for (const changed of this.listeners) this.notifyListener(changed);
  }
  private notifyListener(changed: () => void): void {
    // Inspection is observational: a broken renderer must never interrupt billing or cleanup.
    try { changed(); }
    catch (error) { this.notificationError = `Inspector refresh failed: ${diagnostic(error)}`.slice(0, HELPER_TEXT_LIMIT); }
  }
  private clip(entry: Retained, text: string): string {
    if (text.length > HELPER_TEXT_LIMIT) entry.truncated = true;
    return text.slice(0, HELPER_TEXT_LIMIT);
  }
  begin(request: string, kind: AuxiliaryKind, model: string, prompt: string, serviceTier?: "priority"): void {
    const entry: Retained = { request, kind, model, ...(serviceTier ? { serviceTier } : {}), prompt: "", phase: "preparing", createdAt: this.now(), messages: [], sequence: 0, size: 0, truncated: false };
    entry.prompt = this.clip(entry, prompt);
    this.entries.set(request, entry);
    while (this.entries.size > HELPER_HISTORY_LIMIT) {
      const first = this.entries.keys().next().value;
      if (first !== undefined) this.entries.delete(first);
    }
    this.notify();
  }
  phase(request: string, phase: "queued" | "running"): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    entry.phase = phase;
    if (phase === "running") entry.startedAt = this.now();
    this.notify();
  }
  observe(request: string, event: HelperEvent): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    switch (event.type) {
      case "error": entry.error = this.clip(entry, event.text); break;
      case "inspection_error": entry.inspectionError = this.clip(entry, event.text); break;
      case "assistant_start":
        entry.assistant = `assistant-${++entry.sequence}`;
        entry.assistantLine = "";
        entry.messages.push({ id: entry.assistant, kind: "assistant", text: "", at: this.now() });
        break;
      case "assistant_delta":
      case "assistant_end": {
        let message = entry.messages.find(message => message.id === entry.assistant);
        if (!message) {
          entry.assistant = `assistant-${++entry.sequence}`;
          message = { id: entry.assistant, kind: "assistant", text: "", at: this.now() }; entry.messages.push(message);
        }
        const before = messageSize(message);
        const activity = event.type === "assistant_end" ? event.text : (entry.assistantLine ?? "") + event.text;
        entry.assistantLine = activity.slice(activity.lastIndexOf("\n") + 1).slice(-200);
        const line = activityLine(activity);
        if (line) entry.latestActivity = line;
        message.text = this.clip(entry, event.type === "assistant_end" ? event.text : message.text + event.text);
        if (event.textBlocks) {
          let remaining = HELPER_TEXT_LIMIT;
          if (event.textBlocks.length > 16) entry.truncated = true;
          message.textBlocks = event.textBlocks.slice(0, 16).map(text => {
            if (text.length > remaining) entry.truncated = true;
            const clipped = text.slice(0, remaining); remaining -= clipped.length; return clipped;
          });
        } else if (event.type === "assistant_end") message.textBlocks = undefined;
        if (event.type === "assistant_end") {
          message.stopReason = event.stopReason; message.hasToolCalls = event.hasToolCalls;
          message.errorMessage = event.errorMessage === undefined ? undefined : this.clip(entry, event.errorMessage);
        }
        entry.size += messageSize(message) - before;
        break;
      }
      case "tool": {
        const id = `tool-${createHash("sha256").update(event.id).digest("hex")}`;
        let message = entry.messages.find(message => message.id === id);
        const before = message ? messageSize(message) : 0;
        const tool = event.name.slice(0, 80).replace(/[\r\n\u2028\u2029]/g, " ");
        entry.latestActivity = `${tool}: ${activityLine(event.text ?? "", 198 - tool.length) || event.phase}`;
        if (!message) { message = { id, kind: "tool", tool: this.clip(entry, event.name), text: "", at: this.now() }; entry.messages.push(message); }
        message.phase = event.phase;
        if (event.arguments !== undefined) message.arguments = this.clip(entry, event.arguments);
        if (event.argsComplete !== undefined) message.argsComplete = event.argsComplete;
        if (event.partial !== undefined) message.partial = event.partial;
        if (event.text !== undefined) message.text = this.clip(entry, event.text);
        entry.size += messageSize(message) - before;
        break;
      }
      default: {
        const unreachable: never = event;
        throw new Error(`Unsupported helper activity: ${String(unreachable)}`);
      }
    }
    while (entry.messages.length > 64 || entry.size > TRANSCRIPT_LIMIT) {
      entry.size -= messageSize(entry.messages.shift()!); entry.truncated = true; entry.dropped = (entry.dropped ?? 0) + 1;
    }
    this.notify();
  }
  error(request: string, reason: unknown): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    entry.error = this.clip(entry, diagnostic(reason)); this.notify();
  }
  finish(request: string, outcome: Outcome, result?: string): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    entry.phase = outcome; entry.endedAt = this.now();
    if (result !== undefined) entry.result = this.clip(entry, result);
    this.notify();
  }
  snapshot(): readonly HelperInspection[] {
    return [...this.entries.values()].reverse().map(({ sequence: _sequence, size: _size, assistant: _assistant, assistantLine: _assistantLine, ...entry }) => ({
      ...entry, ...(this.notificationError ? { inspectionError: this.notificationError } : {}), messages: entry.messages.map(message => ({ ...message, ...(message.textBlocks ? { textBlocks: [...message.textBlocks] } : {}) })),
    }));
  }
}
