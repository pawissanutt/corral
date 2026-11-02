import { createHash } from "node:crypto";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type { AuxiliaryKind, Outcome } from "./runtime.ts";

export type HelperEvent =
  | { readonly type: "error" | "inspection_error"; readonly text: string }
  | { readonly type: "assistant_start" }
  | { readonly type: "assistant_delta" | "assistant_end"; readonly text: string; readonly stopReason?: AssistantMessage["stopReason"]; readonly hasToolCalls?: boolean; readonly errorMessage?: string; readonly textBlocks?: readonly string[] }
  | { readonly type: "tool"; readonly id: string; readonly name: string; readonly phase: "requested" | "running" | "completed" | "error"; readonly arguments?: string; readonly text?: string };
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
  readonly phase?: "requested" | "running" | "completed" | "error";
}
export interface HelperInspection {
  readonly request: string; readonly kind: AuxiliaryKind; readonly model: string; readonly generation: number;
  readonly serviceTier?: "priority";
  readonly prompt: string; readonly phase: "preparing" | "queued" | "running" | Outcome;
  readonly createdAt: number; readonly startedAt?: number; readonly endedAt?: number;
  readonly messages: readonly HelperMessage[]; readonly result?: string; readonly error?: string;
  readonly truncated: boolean;
  readonly inspectionError?: string;
}
type MutableMessage = { -readonly [K in keyof HelperMessage]: HelperMessage[K] };
type Retained = Omit<{ -readonly [K in keyof HelperInspection]: HelperInspection[K] }, "messages"> & {
  messages: MutableMessage[]; sequence: number; assistant?: string;
};
export const HELPER_HISTORY_LIMIT = 32;
export const HELPER_TEXT_LIMIT = 4096;
const TRANSCRIPT_LIMIT = 32768;
function diagnostic(error: unknown): string {
  try { return (error instanceof Error ? error.message : String(error)).slice(0, HELPER_TEXT_LIMIT); }
  catch { return "unavailable diagnostic"; }
}

/** In-memory, parent-local UI projection. Never opens session files or changes execution. */
export class AuxiliaryHistory {
  private readonly entries = new Map<string, Retained>();
  private notificationError: string | undefined;
  constructor(private readonly changed: () => void = () => {}) {}
  private notify(): void {
    // Inspection is observational: a broken renderer must never interrupt billing or cleanup.
    try { this.changed(); this.notificationError = undefined; }
    catch (error) { this.notificationError = `Inspector refresh failed: ${diagnostic(error)}`.slice(0, HELPER_TEXT_LIMIT); }
  }
  private clip(entry: Retained, text: string): string {
    if (text.length > HELPER_TEXT_LIMIT) entry.truncated = true;
    return text.slice(0, HELPER_TEXT_LIMIT);
  }
  begin(request: string, kind: AuxiliaryKind, model: string, generation: number, prompt: string, serviceTier?: "priority"): void {
    const entry: Retained = { request, kind, model, generation, ...(serviceTier ? { serviceTier } : {}), prompt: "", phase: "preparing", createdAt: Date.now(), messages: [], sequence: 0, truncated: false };
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
    if (phase === "running") entry.startedAt = Date.now();
    this.notify();
  }
  observe(request: string, event: HelperEvent): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    switch (event.type) {
      case "error": entry.error = this.clip(entry, event.text); break;
      case "inspection_error": entry.inspectionError = this.clip(entry, event.text); break;
      case "assistant_start":
        entry.assistant = `assistant-${++entry.sequence}`;
        entry.messages.push({ id: entry.assistant, kind: "assistant", text: "" });
        break;
      case "assistant_delta":
      case "assistant_end": {
        let message = entry.messages.find(message => message.id === entry.assistant);
        if (!message) {
          entry.assistant = `assistant-${++entry.sequence}`;
          message = { id: entry.assistant, kind: "assistant", text: "" }; entry.messages.push(message);
        }
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
        break;
      }
      case "tool": {
        const id = `tool-${createHash("sha256").update(event.id).digest("hex")}`;
        let message = entry.messages.find(message => message.id === id);
        if (!message) { message = { id, kind: "tool", tool: this.clip(entry, event.name), text: "" }; entry.messages.push(message); }
        message.phase = event.phase;
        if (event.arguments !== undefined) message.arguments = this.clip(entry, event.arguments);
        if (event.text !== undefined) message.text = this.clip(entry, event.text);
        break;
      }
      default: {
        const unreachable: never = event;
        throw new Error(`Unsupported helper activity: ${String(unreachable)}`);
      }
    }
    const size = () => entry.messages.reduce((total, message) => total + Math.max(message.text.length, message.textBlocks?.reduce((sum, text) => sum + text.length, 0) ?? 0) + (message.errorMessage?.length ?? 0) + (message.arguments?.length ?? 0) + (message.tool?.length ?? 0), 0);
    while (entry.messages.length > 64 || size() > TRANSCRIPT_LIMIT) { entry.messages.shift(); entry.truncated = true; }
    this.notify();
  }
  error(request: string, reason: unknown): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    entry.error = this.clip(entry, diagnostic(reason)); this.notify();
  }
  finish(request: string, outcome: Outcome, result?: string): void {
    const entry = this.entries.get(request); if (!entry || entry.endedAt !== undefined) return;
    entry.phase = outcome; entry.endedAt = Date.now();
    if (result !== undefined) entry.result = this.clip(entry, result);
    this.notify();
  }
  snapshot(): readonly HelperInspection[] {
    return [...this.entries.values()].reverse().map(({ sequence: _sequence, assistant: _assistant, ...entry }) => ({
      ...entry, ...(this.notificationError ? { inspectionError: this.notificationError } : {}), messages: entry.messages.map(message => ({ ...message, ...(message.textBlocks ? { textBlocks: [...message.textBlocks] } : {}) })),
    }));
  }
}
