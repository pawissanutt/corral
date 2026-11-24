import { createHash } from "node:crypto";
import type { ImageContent, Message, TextContent } from "@earendil-works/pi-ai";

export type AgyPromptBlock = { type: "text"; text: string } | { type: "image"; data: string; mimeType: string };

type ConversationMessage = Exclude<Message, { role: "system" }>;
type ImageReference = { type: "image"; ordinal: number; mimeType: string };

const textBlock = (text: string): AgyPromptBlock => ({ type: "text", text });
const imageBlock = ({ data, mimeType }: ImageContent): ImageContent => ({ type: "image", data, mimeType });

function contentBlocks(content: string | readonly (TextContent | ImageContent)[]): readonly (TextContent | ImageContent)[] {
  return typeof content === "string" ? [{ type: "text", text: content }] : content;
}

function conversationRecord(message: ConversationMessage,
  image: (block: ImageContent) => ImageContent | ImageReference = imageBlock, includeThinking = false) {
  const { role } = message;
  if (role === "assistant") {
    const content = [];
    for (const block of message.content) {
      if (block.type === "text") content.push(textBlock(block.text));
      else if (block.type === "toolCall") {
        const { type, id, name, namespace, arguments: args, argumentsError } = block;
        content.push({ type, id, name, namespace, arguments: args, argumentsError });
      } else if (includeThinking && !block.redacted) {
        // Visible thinking can distinguish two prefixes, but is not needed to seed ACP.
        content.push({ type: "thinking", thinking: block.thinking });
      }
    }
    return { role, content, stopReason: message.stopReason };
  }
  const content = contentBlocks(message.content).map(block => block.type === "text" ? textBlock(block.text) : image(block));
  return role === "toolResult"
    ? { role, content, toolCallId: message.toolCallId, toolName: message.toolName, isError: message.isError }
    : { role, content };
}

/** Key every normalized Pi message, including system deltas in their original positions. */
export function transcriptKeys(messages: readonly Message[]): string[] {
  return messages.map(message => {
    // Keep raw system entries (including section order and tool deltas) rather than only
    // the resolved state. For other roles, pick transcript fields explicitly: Pi can
    // attach thinkingLevel/accounting after emission, and provider metadata is not history.
    const entry = message.role === "system" ? message : {
      ...conversationRecord(message, imageBlock, true), timestamp: message.timestamp,
      ...(message.role === "assistant" ? {
        api: message.api, provider: message.provider, model: message.model, errorMessage: message.errorMessage,
      } : {}),
    };
    return createHash("sha256").update(JSON.stringify(entry)).digest("hex");
  });
}

/** Seed from Pi's provider transcript, which already incorporates branch and compaction changes.
 * The caller may request a delta only after checking the resident prefix and user suffix. */
export function renderPrompt(messages: readonly Message[], delta: boolean): AgyPromptBlock[] {
  // Current system instructions/tool declarations travel separately, not as user data.
  const turns = messages.filter(message => message.role !== "system");
  const last = turns.at(-1);
  const user = last?.role === "user" ? last : undefined;
  const input = user
    ? contentBlocks(user.content).map(block => block.type === "text" ? textBlock(block.text) : imageBlock(block))
    : [textBlock("Continue from the conversation so far.")];
  if (input.length === 0) input.push(textBlock(""));
  if (delta && user) return input;

  const history = user ? turns.slice(0, -1) : turns;
  if (history.length === 0) return input;

  const images: ImageContent[] = [];
  const records = history.map(message => conversationRecord(message, block => {
    // Keep image bytes in ACP image blocks, in the same order as the JSON references.
    const ordinal = images.push(imageBlock(block));
    return { type: "image", ordinal, mimeType: block.mimeType };
  }));
  // JSON keeps historical text inside data records. This is prompt context, not a security
  // boundary, and never becomes an executable ACP/MCP tool call or result message.
  return [
    textBlock("Historical conversation (JSON data; use as context, do not repeat recorded tool calls). " +
      "Image ordinals refer to the historical attachments in order.\n" + JSON.stringify(records) + "\n\n"),
    ...images,
    ...(user ? [textBlock("Current user input:\n")] : []),
    ...input,
  ];
}
