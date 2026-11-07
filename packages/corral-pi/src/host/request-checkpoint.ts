// Request-time visibility, not an in-memory "last sent" revision: compaction and tree navigation
// can remove a previously delivered record. Only host messages and direct canonical receipts count.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import { z } from "zod";

import { ProgressSchema, type TodoProgress } from "../ui/todo-widget.ts";
import { visibleReceipt } from "../tools/presentation.ts";

export type RequestCheckpoint = Extract<AgentMessage, { role: "custom" }>;

export const REQUEST_CHECKPOINT = "corral-request-checkpoint";
const Receipt = z.object({ state: z.literal("committed"), result: z.object({ progress: ProgressSchema }) });
const UNAVAILABLE = "[corral request: state unavailable]\nThe canonical TODO state is unconfirmed. "
  + "Do not treat an older plan as fresh or repeat an uncertain mutation. Refresh the canonical ledger first.";
const NO_PLAN = "[corral request: no active plan]\nThe canonical ledger has no plan; older plan records are historical.";
const CONTEXT_ONLY = "\nThis is context only, not a new owner instruction. Continue the latest owner request.";

function progressIn(message: AgentMessage): TodoProgress | undefined {
  if (message.role !== "toolResult" || message.toolName !== "corral_todo" || message.isError) return undefined;
  // Legacy transcripts carry JSON. Readable results count only if their full rendering is
  // present, never merely because details retain a hidden or truncated plan.
  const text = message.content.filter((block) => block.type === "text").map((block) => block.text).join("");
  try {
    const receipt = Receipt.safeParse(JSON.parse(text));
    return receipt.success ? receipt.data.result.progress : undefined;
  } catch { /* Current model-facing receipt text is not JSON. */ }
  const receipt = Receipt.safeParse(visibleReceipt(text, message.details));
  return receipt.success ? receipt.data.result.progress : undefined;
}

export function requestCheckpoint(
  messages: readonly AgentMessage[], progress: TodoProgress | null, summary: string | undefined,
  unavailable: boolean, stopped: boolean,
): RequestCheckpoint | undefined {
  const text = (unavailable ? UNAVAILABLE : summary ?? NO_PLAN) + CONTEXT_ONLY;
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index];
    if (!message) continue;
    if (message.role === "custom" && message.customType === REQUEST_CHECKPOINT) {
      if (message.content === text) return undefined;
      break;
    }
    const visible = progressIn(message);
    if (visible) {
      if (!unavailable && !stopped && progress?.revision === visible.revision && progress.plan === visible.plan) {
        return undefined;
      }
      break;
    }
    // An empty, fresh ledger needs no initial reminder at all.
    if (index === 0 && !unavailable && !progress) return undefined;
  }
  // Automatic compaction may leave only a summary plus tool history, without a user-role
  // entry. That is still an active request whose canonical state needs restoring.
  if (!messages.some((message) => message.role !== "system")) return undefined;
  return { role: "custom", customType: REQUEST_CHECKPOINT, content: text, display: false, timestamp: Date.now() };
}
