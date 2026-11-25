import type { TranscriptContext } from "@earendil-works/pi-ai";

/** Keep previews in the durable transcript; only project them out of a text-only provider request. */
export function imageContext(context: TranscriptContext, model: { input?: readonly string[] }): TranscriptContext {
  if (model.input?.includes("image")) return context;
  return { ...context, messages: context.messages.map(message => message.role === "toolResult" && message.toolName === "image_gen"
    ? { ...message, content: message.content.filter(part => part.type !== "image") } : message) };
}
