// The server selects and persists the canonical ledger using the bound identity.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { readAsset } from "./assets.ts";
import { pick } from "./tools.ts";

export const REQUEST_ASSET = "assets/request-execution.md";
export const REQUEST_OFF_ASSET = "assets/request-execution-off.md";

/** The PM and assistant request-execution section for the generation's todo setting. */
export function requestContract(root: string, todo: boolean | undefined): string {
  return readAsset(root, todo === true ? REQUEST_ASSET : REQUEST_OFF_ASSET);
}

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

const Plan = Type.Optional(Type.String({ description: "The plan this update is for, P<n>; an older plan is refused as stale" }));
const SessionStep = Type.Object({ id: Type.String(), label: Type.String() }, { additionalProperties: false });
const Phase = Type.Object({ id: Type.String(), label: Type.String(), steps: Type.Array(SessionStep) },
  { additionalProperties: false });

/** PM and assistant: one session plan per request, rolled into history once it is finished. */
export const sessionTodoTool: CorralToolSpec = {
  name: "corral_todo",
  label: "Update the request plan",
  description: "Keep this session's request plan, which the owner sees and corral restores to you every turn. "
    + "init opens a plan (refused while the current plan has open steps; a finished plan rolls into history). "
    + "append adds steps to a phase, or opens a new phase when you give its label. start, done and drop "
    + "(with a reason) move a step. wait records that an open step waits on a named event or an owner answer; "
    + "starting or finishing the step clears it. Every wait has a deadline (default 2 h for events, "
    + "24 h for the owner), after which corral wakes the session for that step. "
    + "The receipt returns the plan, revision and disposition.",
  op: "todo",
  parameters: Type.Union([
    Type.Object({ op: Type.Literal("init"), phases: Type.Array(Phase) }, { additionalProperties: false }),
    Type.Object({
      op: Type.Literal("append"), phase: Type.String(), steps: Type.Array(SessionStep),
      label: Type.Optional(Type.String({ description: "Opens the phase with this label when it does not exist" })),
      plan: Plan,
    }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("start"), step: Type.String(), plan: Plan }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("done"), step: Type.String(), plan: Plan }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("drop"), step: Type.String(), reason: Type.String(), plan: Plan },
      { additionalProperties: false }),
    Type.Object({
      op: Type.Literal("wait"), step: Type.String(), plan: Plan,
      deadline: Type.Optional(Type.String({ format: "date-time" })),
      on: Type.Union([
        Type.Object({ kind: Type.Literal("event"), event: Type.String({ description: "The observable event" }) },
          { additionalProperties: false }),
        Type.Object({ kind: Type.Literal("owner"), question: Type.String({ description: "The question asked" }) },
          { additionalProperties: false }),
      ]),
    }, { additionalProperties: false }),
  ]),
  payload(params) {
    switch (params.op) {
      case "init": return pick(params, ["op", "phases"]);
      case "append": return pick(params, ["op", "phase", "label", "steps", "plan"]);
      case "start":
      case "done": return pick(params, ["op", "step", "plan"]);
      case "drop": return pick(params, ["op", "step", "reason", "plan"]);
      case "wait": return pick(params, ["op", "step", "on", "plan", "deadline"]);
      default: throw new Error("invalid todo operation");
    }
  },
};
