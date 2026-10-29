// Corral tools of the native planning roles (T-460 plan-roles). corral writes only the file the
// launcher selected for this container (the planner's plan.md, the plan reviewer's notes
// report); the model never names a path, a plan or a review attempt.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

type Params = Record<string, unknown>;

/** Keeps one request's canonical JSON payload well inside corral's 32 KiB bound. */
export const CHUNK_MAX_BYTES = 16 * 1024;

export function planWriteTool(target: "plan" | "notes"): CorralToolSpec {
  const file = target === "plan" ? "the plan file your prompt names" : "your plan review notes report";
  return {
    name: "corral_plan_write",
    label: target === "plan" ? "Write the plan" : "Write review notes",
    description: `Write ${file}; corral picks the path. Send content in chunks of at most `
      + `${CHUNK_MAX_BYTES / 1024} KiB: offset 0 starts the file over, and each next chunk continues at the `
      + "`bytes` value the previous receipt returned. A retried chunk rewrites the same bytes. Read the file "
      + "back with read to check it.",
    op: "plan_write",
    parameters: Type.Object({
      content: Type.String({ description: `The text to write, at most ${CHUNK_MAX_BYTES / 1024} KiB` }),
      offset: Type.Optional(Type.Integer({ minimum: 0, description: "Byte offset to write at; 0 or absent starts over" })),
    }),
    payload: (params: Params) => pick(params, ["content", "offset"]),
  };
}

export const planSubmitTool: CorralToolSpec = {
  name: "corral_plan_submit",
  label: "Submit the plan",
  description: "Submit the plan written with corral_plan_write, exactly as the prompt's `corral plan submit` command "
    + "would: corral snapshots it against the task's current spec. A refused submission (an invalid plan "
    + "document, a stale spec) returns the reason: fix the file and submit again. Submitting is your only "
    + "completion step; review and owner acceptance are separate.",
  op: "plan_submit",
  parameters: Type.Object({}),
  payload: () => ({}),
};
