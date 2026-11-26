import { BridgeRefusal } from "../../bridge/protocol.ts";
import type { PlanMode } from "../plan-mode.ts";
import type { RemoteRegistry } from "./registry.ts";
import { parseCursor } from "./state.ts";

const COMMAND_MAX_BYTES = 16 * 1024;

function reviewCard(card: unknown): string {
  if (typeof card !== "string" || !/^[A-Za-z0-9._:-]{1,160}$/.test(card)) throw new BridgeRefusal("bad_input", "invalid plan review card");
  return card;
}

/** Plan mode controls and read-only queries for a review card. */
export function registerPlanMode(registry: RemoteRegistry, plan: PlanMode): void {
  registry.register({ name: "plan_mode", parse: ({ enabled, command }): { command: string } | { enabled: boolean } => {
    if (typeof command === "string" && enabled === undefined && Buffer.byteLength(command) <= COMMAND_MAX_BYTES) return { command };
    if (typeof enabled !== "boolean" || command !== undefined) throw new BridgeRefusal("bad_input", "plan_mode takes enabled: true or false");
    return { enabled };
  }, run: async input => ({
    text: "command" in input ? (await plan.command(input.command)).text : input.enabled ? await plan.enter() : plan.leave(),
  }) });
  registry.register({ name: "plan_mode_doc", parse: ({ card, after }) => ({ card: reviewCard(card), after: parseCursor(after) }),
    run: ({ card, after }) => plan.doc(card, after) });
  registry.register({ name: "plan_status", parse: ({ card }) => reviewCard(card), run: card => ({ plan_status: plan.status(card) }) });
}
