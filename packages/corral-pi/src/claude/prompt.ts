// Renders Pi's transcript into the single user message one per-turn query sends (T-493), ported
// from senpi's prompt-bridge.js and content-blocks.js (MIT). Earlier turns become a labelled,
// non-executable history; the final user message follows as the live request.
import type { SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { getCurrentSystemMessage, type AssistantMessage, type ImageContent, type Message, type TextContent } from "@earendil-works/pi-ai";

import { historicalToolName } from "./tools.ts";
import { DEFAULT_CACHE_TTL, type ClaudeCacheRetention, type PromptCacheTtl } from "./cache-policy.ts";

export const PROMPT_CACHE_TTL = DEFAULT_CACHE_TTL;

export type PromptBlock =
  | { type: "text"; text: string; cache_control?: { type: "ephemeral"; ttl: PromptCacheTtl } }
  | { type: "image"; source: { type: "base64"; media_type: "image/jpeg" | "image/png" | "image/gif" | "image/webp"; data: string } };

const IMAGE_MEDIA_TYPES = ["image/jpeg", "image/png", "image/gif", "image/webp"] as const;

/** Appends `content`; true when it carried text the model can read (false for image-only content). */
function appendContent(blocks: PromptBlock[], content: string | readonly (TextContent | ImageContent)[]): boolean {
  if (typeof content === "string") {
    if (content.length > 0) blocks.push({ type: "text", text: content });
    return content.trim().length > 0;
  }
  let hasText = false;
  for (const entry of content) {
    if (entry.type === "text") {
      blocks.push({ type: "text", text: entry.text });
      hasText = entry.text.trim().length > 0 || hasText;
      continue;
    }
    const mediaType = IMAGE_MEDIA_TYPES.find((candidate) => candidate === entry.mimeType);
    if (mediaType) {
      blocks.push({ type: "image", source: { type: "base64", media_type: mediaType, data: entry.data } });
      // SDK image breakpoints are dropped. A stable text boundary lets the rolling cache
      // frontier traverse even a batch containing more than twenty images.
      blocks.push({ type: "text", text: "[attached image]" });
    } else {
      blocks.push({ type: "text", text: `[image block omitted: unsupported media type ${entry.mimeType}]` });
      hasText = true;
    }
  }
  return hasText;
}

function assistantText(message: AssistantMessage): string {
  return message.content.map((block) => {
    if (block.type === "text") return block.text;
    if (block.type === "thinking") return block.thinking;
    return `Historical tool call (non-executable): ${historicalToolName(block.name)} ` +
      `args=${JSON.stringify(block.arguments)}`;
  }).join("\n");
}

/** Seal each user/assistant message and parallel result batch; never merge into earlier sealed text. */
function sealHistory(blocks: PromptBlock[], parts: PromptBlock[]): void {
  let text: string[] = [];
  const flush = () => {
    if (text.length > 0) blocks.push({ type: "text", text: text.join("") });
    text = [];
  };
  for (const part of parts) {
    if (part.type === "text") text.push(part.text);
    else { flush(); blocks.push(part); }
  }
  flush();
  parts.length = 0;
}

export function buildPromptBlocks(
  messages: readonly Message[], _customToolNameToSdk: ReadonlyMap<string, string>,
  retention: ClaudeCacheRetention = DEFAULT_CACHE_TTL,
): PromptBlock[] {
  const blocks: PromptBlock[] = [];
  const pushText = (text: string) => blocks.push({ type: "text", text });
  const turns = messages.filter((message) => message.role !== "system");
  const last = turns.at(-1);
  const finalUser = last?.role === "user" ? last : undefined;
  const history = finalUser ? turns.slice(0, -1) : turns;
  if (history.length > 0) {
    pushText("<conversation_history>\n");
    const parts: PromptBlock[] = [];
    let previous = false;
    const prefix = (label: string) => {
      parts.push({ type: "text", text: `${previous ? "\n\n" : ""}${label}\n` });
      previous = true;
    };
    for (const message of history) {
      // A new assistant/user seals any previous batch. Its arrival must not rewrite that batch's prefix.
      if (message.role !== "toolResult") sealHistory(blocks, parts);
      if (message.role === "user") {
        prefix("USER:");
        if (!appendContent(parts, message.content)) parts.push({ type: "text", text: "(see attached image)" });
      } else if (message.role === "assistant") {
        prefix("ASSISTANT:");
        const text = assistantText(message);
        if (text.length > 0) parts.push({ type: "text", text });
      } else if (message.role === "toolResult") {
        prefix(`TOOL RESULT (historical ${historicalToolName(message.toolName)}, ` +
          `id=${message.toolCallId}):`);
        if (!appendContent(parts, message.content)) parts.push({ type: "text", text: "(see attached image)" });
      }
      if (message.role !== "toolResult") sealHistory(blocks, parts);
    }
    sealHistory(blocks, parts);
    // Claude Code drops image breakpoints. Cache the last nonempty text prefix before the
    // moving closer/instruction, so the next replay can find this write (senpi prompt-bridge).
    for (let index = blocks.length - 1; index >= 0; index--) {
      const block = blocks[index];
      if (retention === "none") break;
      if (block?.type !== "text" || !block.text.trim()) continue;
      blocks[index] = { ...block, cache_control: { type: "ephemeral", ttl: retention } };
      break;
    }
    pushText("\n</conversation_history>");
  }
  // Current host state belongs after the reusable history, not in the root system/cache prefix.
  const questionState = getCurrentSystemMessage(messages)?.sections?.owner_questions;
  if (typeof questionState === "string") pushText(`<host_question_state>\n${questionState}\n</host_question_state>`);
  pushText("The above is the conversation history so far, provided as context. Respond as the assistant to the user " +
    "message below only. Never emit \"USER:\" or \"ASSISTANT:\" labels or continue the transcript.");
  if (finalUser && !appendContent(blocks, finalUser.content)) pushText("(see attached image)");
  return blocks;
}

export async function* promptStream(blocks: PromptBlock[]): AsyncGenerator<SDKUserMessage> {
  yield {
    type: "user",
    message: { role: "user", content: blocks },
    parent_tool_use_id: null,
    session_id: "prompt",
  } as SDKUserMessage;
}
