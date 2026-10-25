// The reviewer-only corral tool: verdict. The parent run, review child and callback token it acts
// on are launcher-bound; corral's Rust operation table stays the authority on what runs.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

type Params = Record<string, unknown>;

const GateRef = Type.Object({
  attempt: Type.String(),
  tier: Type.String(),
  receipt_sha256: Type.String(),
}, { description: "The run's gate_ref exactly as `corral show <task> --json` reports it" });

export const verdictTool: CorralToolSpec = {
  name: "corral_verdict",
  label: "Record the review verdict",
  description: "Record this review's verdict, exactly as the review prompt's `corral verdict` command would, for "
    + "the attempt corral launched you for. approve: the candidate meets the spec and its gate tier. changes: "
    + "list each required change in the notes. escalate: the owner must decide. It refuses while any of your "
    + "questions is pending or an answer is unread (wait for it or withdraw the question), and when the candidate "
    + "changed since the review started (review_closed: stop, the review is stale).",
  op: "verdict",
  parameters: Type.Object({
    verdict: Type.Union([Type.Literal("approve"), Type.Literal("changes"), Type.Literal("escalate")]),
    notes: Type.String({ description: "The review notes in Markdown: findings, evidence and required changes" }),
    gate_ref: Type.Optional(GateRef),
  }),
  payload: (params: Params) => pick(params, ["verdict", "notes", "gate_ref"]),
};
