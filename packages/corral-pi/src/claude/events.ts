// Maps Claude Code's streamed Anthropic events onto Pi's assistant message stream (T-493), ported
// from senpi's stream-events.js, stream-protocol.js and errors.js (MIT).
import {
  type AssistantMessage, type AssistantMessageEventStream, type Model, type StopReason, type TextContent,
  type ThinkingContent, type ToolCall, calculateCost, parseStreamingJson,
} from "@earendil-works/pi-ai";

import { mapSdkToolNameToPi, mapToolArgs } from "./tools.ts";

export interface SdkUsage {
  input_tokens?: number | null;
  output_tokens?: number | null;
  cache_read_input_tokens?: number | null;
  cache_creation_input_tokens?: number | null;
}

/** The subset of Anthropic's raw stream events Claude Code forwards as `stream_event`. */
export type StreamEvent =
  | { type: "message_start"; message: { usage?: SdkUsage } }
  | { type: "content_block_start"; index: number; content_block: { type: string; id?: string; name?: string; input?: unknown } }
  | { type: "content_block_delta"; index: number; delta: { type: string; text?: string; thinking?: string; signature?: string; partial_json?: string } }
  | { type: "content_block_stop"; index: number }
  | { type: "message_delta"; delta: { stop_reason?: string | null }; usage?: SdkUsage }
  | { type: "message_stop" };

type Indexed<T> = T & { index?: number; partialJson?: string };
type OpenBlock = Indexed<TextContent> | Indexed<ThinkingContent> | Indexed<ToolCall>;

export interface EventContext {
  readonly model: Model<string>;
  readonly output: AssistantMessage;
  readonly blocks: OpenBlock[];
  readonly stream: AssistantMessageEventStream;
  readonly customToolNameToPi: ReadonlyMap<string, string>;
}

export function mapStopReason(reason: string | null | undefined): StopReason {
  if (reason === "tool_use") return "toolUse";
  if (reason === "max_tokens") return "length";
  return "stop";
}

export function updateUsage(model: Model<string>, output: AssistantMessage, usage: SdkUsage | undefined): void {
  if (!usage) return;
  if (usage.input_tokens != null) output.usage.input = usage.input_tokens;
  if (usage.output_tokens != null) output.usage.output = usage.output_tokens;
  if (usage.cache_read_input_tokens != null) output.usage.cacheRead = usage.cache_read_input_tokens;
  if (usage.cache_creation_input_tokens != null) output.usage.cacheWrite = usage.cache_creation_input_tokens;
  output.usage.totalTokens = output.usage.input + output.usage.output + output.usage.cacheRead + output.usage.cacheWrite;
  calculateCost(model, output.usage);
}

export function emptyOutput(model: Model<string>): AssistantMessage {
  return {
    role: "assistant", content: [], api: model.api, provider: model.provider, model: model.id,
    usage: {
      input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: "stop", timestamp: Date.now(),
  };
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function open(context: EventContext, block: OpenBlock, kind: "text" | "thinking" | "toolcall"): void {
  context.blocks.push(block);
  context.output.content.push(block);
  context.stream.push({ type: `${kind}_start`, contentIndex: context.output.content.length - 1, partial: context.output });
}

function find(context: EventContext, index: number): { block: OpenBlock | undefined; contentIndex: number } {
  const block = context.blocks.find((candidate) => candidate.index === index);
  return { block, contentIndex: block ? context.output.content.indexOf(block) : -1 };
}

export function applyStreamEvent(context: EventContext, event: StreamEvent): void {
  const { output, stream } = context;
  if (event.type === "message_start") {
    updateUsage(context.model, output, event.message.usage);
  } else if (event.type === "content_block_start") {
    const start = event.content_block;
    if (start.type === "text") open(context, { type: "text", text: "", index: event.index }, "text");
    else if (start.type === "thinking") {
      open(context, { type: "thinking", thinking: "", thinkingSignature: "", index: event.index }, "thinking");
    } else if (start.type === "tool_use") {
      open(context, {
        type: "toolCall", id: start.id ?? "", name: mapSdkToolNameToPi(start.name ?? "", context.customToolNameToPi),
        arguments: {}, partialJson: "", index: event.index,
      }, "toolcall");
    }
  } else if (event.type === "content_block_delta") {
    const { block, contentIndex } = find(context, event.index);
    const delta = event.delta;
    if (delta.type === "text_delta" && block?.type === "text" && delta.text !== undefined) {
      block.text += delta.text;
      stream.push({ type: "text_delta", contentIndex, delta: delta.text, partial: output });
    } else if (delta.type === "thinking_delta" && block?.type === "thinking" && delta.thinking !== undefined) {
      block.thinking += delta.thinking;
      stream.push({ type: "thinking_delta", contentIndex, delta: delta.thinking, partial: output });
    } else if (delta.type === "signature_delta" && block?.type === "thinking" && delta.signature !== undefined) {
      block.thinkingSignature = `${block.thinkingSignature ?? ""}${delta.signature}`;
    } else if (delta.type === "input_json_delta" && block?.type === "toolCall" && delta.partial_json !== undefined) {
      block.partialJson = `${block.partialJson ?? ""}${delta.partial_json}`;
      stream.push({ type: "toolcall_delta", contentIndex, delta: delta.partial_json, partial: output });
    }
  } else if (event.type === "content_block_stop") {
    const { block, contentIndex } = find(context, event.index);
    if (block?.type === "text") stream.push({ type: "text_end", contentIndex, content: block.text, partial: output });
    else if (block?.type === "thinking") {
      stream.push({ type: "thinking_end", contentIndex, content: block.thinking, partial: output });
    } else if (block?.type === "toolCall") {
      block.arguments = mapToolArgs(block.name, record(parseStreamingJson(block.partialJson))) as ToolCall["arguments"];
      delete block.partialJson;
      stream.push({ type: "toolcall_end", contentIndex, toolCall: block, partial: output });
    }
    if (block) delete block.index;
  } else if (event.type === "message_delta") {
    output.stopReason = mapStopReason(event.delta.stop_reason);
    updateUsage(context.model, output, event.usage);
  }
}

interface ResultMessage {
  subtype: string;
  is_error?: boolean;
  errors?: unknown[];
  result?: unknown;
  api_error_status?: number | null;
  terminal_reason?: unknown;
}

export function resultFailure(message: ResultMessage): Error | undefined {
  if (message.subtype === "success" && message.is_error !== true) return undefined;
  const firstError = (message.errors ?? []).find((error): error is string => typeof error === "string" && error.length > 0);
  const resultText = typeof message.result === "string" ? message.result.trim() : "";
  const detail = firstError ?? (message.is_error === true && resultText ? resultText : `Claude Code ${message.subtype}`);
  const suffix = [message.api_error_status != null ? `HTTP ${message.api_error_status}` : "",
    typeof message.terminal_reason === "string" ? message.terminal_reason : ""].filter(Boolean).join(", ");
  return new Error(suffix ? `${detail} (${suffix})` : detail);
}

interface AssistantSdkMessage {
  error?: string;
  message: { content: string | readonly { type: string; text?: unknown }[]; stop_reason?: string | null };
}

/** An assistant message Claude Code marked as failed (auth, billing, rate limit, refusal). */
export function assistantFailure(message: AssistantSdkMessage): Error | undefined {
  if (message.message.stop_reason === "refusal") return new Error("Claude refused this request");
  if (!message.error) return undefined;
  const content = message.message.content;
  const text = (typeof content === "string" ? content
    : content.filter((block) => block.type === "text" && typeof block.text === "string").map((block) => block.text).join(" "))
    .trim();
  if (!text) return new Error(message.error);
  return new Error(message.error === "unknown" ? text : `${text} (${message.error})`);
}
