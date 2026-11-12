// Typed board actions for the PM. Rust rechecks policy, scope, hold and task identity at the
// mutation; this schema only selects one closed action and never carries a command string.
import { Type } from "typebox";
import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

/** `act` runs one scoped sync (dispatch can launch a worker), longer than a plain read. */
export const ACTION_TIMEOUT_MS = 10 * 60 * 1000;

export const actionTool: CorralToolSpec = {
  name: "corral_action",
  label: "Board action",
  description: "Take one board action through corral; prefer this over `bash` for board actions. "
    + "act: execute one executable option, by the `id` of its card command (`... pm act <id> --json`) from "
    + "corral_next or corral_brief; corral revalidates it against fresh policy, scope, hold and task identity "
    + "and never runs owner-required options. "
    + "park: stop new execution of one task with a reason and machine-checkable resume_when checks "
    + "(status:<task>=<status>, landed:<task>, answered:<key>@<rev>, decision:<id>, hold-lifted:<task|project>); "
    + "review_by is 1h..7d (default 24h); an owner park is never replaced. "
    + "attend: record handling of one current attention key with a note; it never answers or moves the task. "
    + "A refusal is final for that request and ID: re-read corral_next instead of retrying it.",
  op: "pm_action",
  parameters: Type.Union([
    Type.Object({
      action: Type.Literal("act"),
      id: Type.String({ description: "The executable action ID from a corral_next or corral_brief card command" }),
    }, { additionalProperties: false }),
    Type.Object({
      action: Type.Literal("park"),
      task: Type.String({ description: "One task ID in an approved project" }),
      reason: Type.String(),
      resume_when: Type.Optional(Type.Array(Type.String(), { maxItems: 8 })),
      resume_action: Type.Optional(Type.String()),
      review_by: Type.Optional(Type.String({ pattern: "^[0-9]+[smhd]$", description: "Review interval, 1h..7d; default 24h" })),
    }, { additionalProperties: false }),
    Type.Object({
      action: Type.Literal("attend"),
      key: Type.String({ description: "One current attention key (<task>/...)" }),
      note: Type.String(),
    }, { additionalProperties: false }),
  ]),
  payload(params) {
    return pick(params, ["action", "id", "task", "reason", "resume_when", "resume_action", "review_by", "key", "note"]);
  },
  timeoutMs: ACTION_TIMEOUT_MS,
};
