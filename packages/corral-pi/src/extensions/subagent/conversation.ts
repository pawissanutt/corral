import type { AgentToolResult } from "@earendil-works/pi-coding-agent";

/** Execution still rejects; the wrapping tool can record the failure alongside its readback reference. */
export class RetainedConversationError extends Error {
  constructor(readonly conversationId: string, cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
  }
}

/** A bounded projection for renderers and nested-call journals, not arbitrary tool details. */
export function helperConversationId(tool: string, value: unknown): string | undefined {
  // Retired oracle calls can still link to their recorded conversations.
  if (!["explore", "oracle", "corral_expert"].includes(tool)) return;
  if (typeof value === "string" && /^[a-z0-9-]{1,36}$/.test(value)) return value;
}

export function retainedToolFailure(error: unknown): AgentToolResult<{ conversationId: string; error: string }> {
  if (!(error instanceof RetainedConversationError)) throw error;
  return { content: [{ type: "text", text: error.message }],
    details: { conversationId: error.conversationId, error: error.message }, isError: true };
}
