// A selected deferred answer uses the same authenticated owner-input operation as a blocking
// dialog. Persist its event identity before the call, so a lost receipt is replayed, not reissued.
import type { SessionManager } from "@earendil-works/pi-coding-agent";
import type { EventLog } from "../bridge/events.ts";
import { type CorralClient, eventRequestId, sha256Hex } from "../tools/corral.ts";
import { ownerInputText } from "../roles/assistant/owner-action.ts";

const INTENT = "corral_question_owner_input";
export async function recordQuestionOwnerInput(options: {
  manager: SessionManager; client: CorralClient; events: EventLog;
  generation: number; turn: number | undefined; question: string; text: string;
}): Promise<string> {
  const { manager, client, events, generation, question, text } = options;
  const prior = manager.getEntries().findLast(entry => entry.type === "custom" && entry.customType === INTENT
    && (entry.data as { question?: unknown; generation?: unknown })?.question === question
    && (entry.data as { generation?: unknown })?.generation === generation);
  const saved = prior?.type === "custom" ? prior.data as { request: string; turn?: number; sha256: string } : undefined;
  const sha256 = sha256Hex(text);
  if (saved && saved.sha256 !== sha256) throw new Error("question_owner_input_conflict");
  const event = saved ? undefined : events.emit("owner_input", { source: "dialog", question, sha256, bytes: Buffer.byteLength(text) });
  const intent = saved ?? { request: eventRequestId(generation, event!.seq), turn: options.turn, sha256 };
  if (!saved) manager.appendCustomEntry(INTENT, { ...intent, question, generation });
  const receipt = await client.call("event", { requestId: intent.request,
    payload: { kind: "owner_input", source: "dialog", text, sha256 },
    ...(intent.turn === undefined ? {} : { turn: intent.turn }),
  });
  const input = (receipt.result as { input?: unknown } | undefined)?.input;
  if (receipt.state !== "committed" || typeof input !== "string") throw new Error("question_owner_input_unjournaled");
  return ownerInputText(text, input);
}
