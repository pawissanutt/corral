// The server selects and persists the canonical ledger using the bound identity.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "./tools.ts";

const Step = Type.Object({
  id: Type.String(),
  label: Type.String(),
  criteria: Type.Optional(Type.Array(Type.String())),
}, { additionalProperties: false });

export const todoTool: CorralToolSpec = {
  name: "corral_todo",
  label: "Update the todo ledger",
  description: "Update your canonical phase and step ledger. Initialize phases, append steps to a phase, "
    + "start or finish a step, or drop a step with a reason. Use stable phase and step IDs. "
    + "The server selects the ledger and returns its revision and full canonical progress in the receipt.",
  op: "todo",
  parameters: Type.Union([
    Type.Object({
      op: Type.Literal("init"),
      phases: Type.Array(Type.Object({
        id: Type.String(),
        label: Type.String(),
        steps: Type.Array(Step),
      }, { additionalProperties: false })),
    }, { additionalProperties: false }),
    Type.Object({
      op: Type.Literal("append"),
      phase: Type.String(),
      steps: Type.Array(Step),
    }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("start"), step: Type.String() }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("done"), step: Type.String() }, { additionalProperties: false }),
    Type.Object({
      op: Type.Literal("drop"),
      step: Type.String(),
      reason: Type.String(),
    }, { additionalProperties: false }),
  ]),
  payload(params) {
    switch (params.op) {
      case "init": return pick(params, ["op", "phases"]);
      case "append": return pick(params, ["op", "phase", "steps"]);
      case "start":
      case "done": return pick(params, ["op", "step"]);
      case "drop": return pick(params, ["op", "step", "reason"]);
      default: throw new Error("invalid todo operation");
    }
  },
};
