import type { Message } from "@earendil-works/pi-ai";
import { z } from "zod";
import { ProgressSchema } from "../../src/ui/todo-widget.ts";
import { visibleReceipt } from "../../src/tools/presentation.ts";

const Receipt = z.object({ state: z.literal("committed"), result: z.object({ progress: ProgressSchema }) });

/** Inspect the actual model transcript, not just its final user message or the host's ledger. */
export function modelPlan(messages: readonly Message[]) {
  for (const message of messages.toReversed()) {
    if (message.role === "system" || message.role === "assistant") continue;
    const text = typeof message.content === "string" ? message.content
      : message.content.filter((block) => block.type === "text").map((block) => block.text).join("");
    if (message.role === "toolResult" && message.toolName === "corral_todo") {
      let value: unknown;
      try { value = JSON.parse(text); }
      catch { value = visibleReceipt(text, message.details); }
      const receipt = Receipt.safeParse(value);
      if (receipt.success) return { plan: receipt.data.result.progress.plan,
        revision: receipt.data.result.progress.revision, text, progress: receipt.data.result.progress };
    }
    const checkpoint = /\[corral request (P\d+) revision (\d+):/.exec(text);
    if (message.role === "user" && checkpoint) {
      return { plan: checkpoint[1], revision: Number(checkpoint[2]), text, progress: undefined };
    }
  }
  return undefined;
}
