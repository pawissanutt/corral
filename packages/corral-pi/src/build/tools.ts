import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { validatePlan } from "./board.ts";
import type { BoardPlan } from "./types.ts";

export function buildFinishTool(open: () => void): ToolDefinition {
  return {
    name: "build_finish", label: "Offer the build exit",
    description: "Tell the owner the build branch is ready for an exit decision. "
      + "Optionally summarize the completed work. This only opens the owner's choices; it does not submit, waive review or land anything. "
      + "After calling this tool, stop taking landing actions and let the owner choose.",
    parameters: Type.Object({ summary: Type.Optional(Type.String()) }, { additionalProperties: false }),
    async execute(_id, _params: { summary?: string }) {
      open();
      return { content: [{ type: "text", text: "The owner now chooses the build exit. Do not act further on landing, submit the branch, waive review or run corral sync yourself." }], details: {} };
    },
  };
}

export function boardPlanTool(propose: (plan: BoardPlan) => void, available: () => string | undefined): ToolDefinition {
  return {
    name: "board_plan", label: "Propose board tasks",
    description: "Propose a bounded plan of at most 20 follow-on tasks for the owner to confirm. "
      + "Give each task a unique key, a concrete title and body, and dependencies naming local keys or existing T-<n> tasks. "
      + "Dependencies must be acyclic. This tool previews intent only; it never imports or queues tasks and grants no board authority.",
    parameters: Type.Object({
      title: Type.String({ minLength: 1 }), project: Type.Optional(Type.String()), why: Type.String(),
      tasks: Type.Array(Type.Object({
        key: Type.String({ minLength: 1 }), title: Type.String({ minLength: 1 }),
        kind: Type.Optional(Type.String()), size: Type.Optional(Type.String()), profile: Type.Optional(Type.String()),
        depends: Type.Array(Type.String()), body: Type.String(),
      }, { additionalProperties: false }), { minItems: 1, maxItems: 20 }),
    }, { additionalProperties: false }),
    async execute(_id, plan: BoardPlan) {
      const unavailable = available();
      if (unavailable) return { content: [{ type: "text", text: unavailable }], details: {}, isError: true };
      try {
        validatePlan(plan);
        propose(plan);
        return { content: [{ type: "text", text: "The owner will confirm this board plan before any task is filed or queued." }], details: {} };
      } catch (error) {
        return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], details: {}, isError: true };
      }
    },
  };
}
