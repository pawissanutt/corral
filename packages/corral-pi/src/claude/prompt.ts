// Renders Pi's transcript into the single user message one per-turn query sends (T-493), ported
// from senpi's prompt-bridge.js and content-blocks.js (MIT). Earlier turns become a labelled,
// non-executable history; the final user message follows as the live request.
import type { SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { AssistantMessage, ImageContent, Message, TextContent } from "@earendil-works/pi-ai";

import { mapPiToolNameToSdk } from "./tools.ts";

export type PromptBlock =
  | { type: "text"; text: string }
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
    } else {
      blocks.push({ type: "text", text: `[image block omitted: unsupported media type ${entry.mimeType}]` });
      hasText = true;
    }
  }
  return hasText;
}

function assistantText(message: AssistantMessage, customToolNameToSdk: ReadonlyMap<string, string>): string {
  return message.content.map((block) => {
    if (block.type === "text") return block.text;
    if (block.type === "thinking") return block.thinking;
    return `Historical tool call (non-executable): ${mapPiToolNameToSdk(block.name, customToolNameToSdk)} ` +
      `args=${JSON.stringify(block.arguments)}`;
  }).join("\n");
}

export function buildPromptBlocks(messages: readonly Message[], customToolNameToSdk: ReadonlyMap<string, string>): PromptBlock[] {
  const blocks: PromptBlock[] = [];
  const pushText = (text: string) => blocks.push({ type: "text", text });
  const turns = messages.filter((message) => message.role !== "system");
  const last = turns.at(-1);
  const finalUser = last?.role === "user" ? last : undefined;
  const history = finalUser ? turns.slice(0, -1) : turns;
  if (history.length > 0) {
    pushText("<conversation_history>\n");
    let previous = false;
    const prefix = (label: string) => {
      pushText(`${previous ? "\n\n" : ""}${label}\n`);
      previous = true;
    };
    for (const message of history) {
      if (message.role === "user") {
        prefix("USER:");
        if (!appendContent(blocks, message.content)) pushText("(see attached image)");
      } else if (message.role === "assistant") {
        prefix("ASSISTANT:");
        const text = assistantText(message, customToolNameToSdk);
        if (text.length > 0) pushText(text);
      } else if (message.role === "toolResult") {
        prefix(`TOOL RESULT (historical ${mapPiToolNameToSdk(message.toolName, customToolNameToSdk)}, ` +
          `id=${message.toolCallId}):`);
        if (!appendContent(blocks, message.content)) pushText("(see attached image)");
      }
    }
    pushText("\n</conversation_history>");
  }
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
