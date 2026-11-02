// A nested result is observable only inside the authenticated Code Mode child that executed it.
// A parent ID or receipt-shaped object alone cannot grant provenance to an arbitrary tool.
import { nestedChild } from "../extensions/codemode/child.ts";

export function observedCorralResult(event: {
  readonly toolName: string;
  readonly toolCallId: string;
  readonly parentToolCallId?: string;
}): boolean {
  if (event.parentToolCallId === undefined) return true;
  const child = nestedChild(event.toolName, event.toolCallId);
  return child !== undefined && child.denied === undefined
    && child.parent.tool_call_id === event.parentToolCallId;
}
