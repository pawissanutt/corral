// The plan reviewer's verdict tool. The plan and review attempt it acts on are launcher-bound,
// so a launched reviewer's verdict is never an owner verdict and never accepts a plan.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

type Params = Record<string, unknown>;

export const planVerdictTool: CorralToolSpec = {
  name: "corral_plan_verdict",
  label: "Record the plan verdict",
  description: "Record this plan review's verdict, exactly as the prompt's `corral plan verdict` command would, for "
    + "the plan and attempt corral launched you for. approve: the plan is ready for owner acceptance (an "
    + "approval never accepts it). changes: list each required change in the notes. escalate: the owner must "
    + "decide. notes become your notes report; omit them only after writing long notes with corral_plan_write.",
  op: "plan_verdict",
  parameters: Type.Object({
    verdict: Type.Union([Type.Literal("approve"), Type.Literal("changes"), Type.Literal("escalate")]),
    notes: Type.Optional(Type.String({ description: "The review notes in Markdown, at most 16 KiB" })),
  }),
  payload: (params: Params) => pick(params, ["verdict", "notes"]),
};
