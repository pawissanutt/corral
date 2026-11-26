// Record a verdict for one immutable request in the bound reviewer session.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";
import { REVIEW_REQUEST_PATTERN } from "../../tools/verdict-intent.ts";

export const verdictTool: CorralToolSpec = {
  name: "corral_verdict",
  label: "Record the review verdict",
  description: "Record one verdict for the candidate you reviewed. Copy its exact request ID from the engine's review prompt "
    + "or re-review message. Approve when the candidate meets the spec; request changes with each required fix and its supporting evidence in the notes.",
  op: "verdict",
  parameters: Type.Object({
    request: Type.String({ pattern: REVIEW_REQUEST_PATTERN, description: "Copy the exact Review request ID for the reviewed candidate" }),
    verdict: Type.Union([Type.Literal("approve"), Type.Literal("changes")]),
    notes: Type.String({ description: "The review notes in Markdown: findings, evidence and required changes" }),
  }, { additionalProperties: false }),
  payload: (params) => pick(params, ["request", "verdict", "notes"]),
};
