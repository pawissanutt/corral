// Record a verdict against the task bound to the reviewer session.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

export const verdictTool: CorralToolSpec = {
  name: "corral_verdict",
  label: "Record the review verdict",
  description: "Record one verdict for the current review round. Approve when the candidate meets the spec; "
    + "request changes with each required fix and its supporting evidence in the notes.",
  op: "verdict",
  parameters: Type.Object({
    verdict: Type.Union([Type.Literal("approve"), Type.Literal("changes")]),
    notes: Type.String({ description: "The review notes in Markdown: findings, evidence and required changes" }),
  }, { additionalProperties: false }),
  payload: (params) => pick(params, ["verdict", "notes"]),
};
