import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { ChildSpec } from "../../build/types.ts";
import type { FleetRuntime } from "./runtime.ts";

/** Model-callable controls only; exit and board decisions remain owner-only. */
export function fleetTools(runtime: FleetRuntime): ToolDefinition[] {
  return [
    {
      name: "task", label: "Delegate build slices",
      description: "Delegate independent write-capable slices in this shared build worktree. Give each child a complete brief and owned paths; no conversation is inherited. "
        + "At most twelve children per call; concurrency follows the configured cap and ready children run FIFO. Use after to wait for successful dependencies. "
        + "Children cannot delegate or write Git state. This call waits for every child to finish and returns reports, edited files, usage and failures; edits outside ownership are flagged, not blocked.",
      promptSnippet: "Delegate bounded implementation slices to write-capable children in the shared worktree",
      promptGuidelines: [
        "Use task for substantial independent implementation, not trivial edits. Give children non-overlapping owned files, the full requirements and expected checks; they inherit none of your conversation.",
        "Keep useful separate work for yourself. Use after only for real dependencies, and unique lowercase ids for the entire session. Choose a configured profile or an approved provider/model.",
        "Collect and reconcile every child report and outside-ownership edit before claiming completion. Do not run shared checks while children are still editing. Children grant no review or board authority.",
      ],
      parameters: Type.Object({ agents: Type.Array(Type.Object({
        id: Type.String({ pattern: "^[a-z][a-z0-9-]{0,31}$" }), title: Type.String({ minLength: 1 }),
        brief: Type.String({ minLength: 1, maxLength: 32768 }), owns: Type.Array(Type.String()),
        profile: Type.Optional(Type.String()), after: Type.Optional(Type.Array(Type.String())),
      }, { additionalProperties: false }), { minItems: 1, maxItems: 12 }) }, { additionalProperties: false }),
      async execute(id, params: { agents: ChildSpec[] }, signal) {
        const children = await runtime.start(id, params.agents, signal);
        const text = children.map(child => {
          const files = child.edits.length ? child.edits.map(edit => `- ${edit.path}${edit.outside ? " [outside owns]" : ""}`).join("\n") : "- none";
          const usage = [`${child.usage.input} input`, `${child.usage.output} output`, `${child.usage.turns} turns`,
            ...(child.usage.cost === undefined ? [] : [`$${child.usage.cost.toFixed(6)}`]),
            ...(child.usage.contextPercent === undefined ? [] : [`${child.usage.contextPercent.toFixed(1)}% context`])].join(", ");
          return `## ${child.id}: ${child.state}\nFiles edited:\n${files}\nUsage: ${usage}${child.error ? `\nError: ${child.error}` : ""}\n\n${child.report ?? "No report."}`;
        }).join("\n\n");
        return { content: [{ type: "text", text }], details: { children }, isError: children.length > 0 && children.every(child => child.state === "failed") };
      },
    },
    {
      name: "agent_message", label: "Steer a build child",
      description: "Send a live child a concise correction or missing context for its next step. Returns whether the message was delivered; finished or queued children cannot receive steering.",
      promptSnippet: "Send a correction or missing context to a running build child",
      promptGuidelines: ["Use agent_message to correct a live child's brief without duplicating its work. A delivered message is not a completion report; collect the task result."],
      parameters: Type.Object({ id: Type.String(), text: Type.String({ minLength: 1 }) }, { additionalProperties: false }),
      async execute(_id, params: { id: string; text: string }) {
        const delivered = await runtime.steer(params.id, params.text);
        return { content: [{ type: "text", text: delivered ? `Message delivered to ${params.id}.` : `Child ${params.id} is not running; no message delivered.` }], details: { delivered } };
      },
    },
    {
      name: "agent_cancel", label: "Cancel a build child",
      description: "Cancel one queued or running child without cancelling unrelated slices. Its dependents are cancelled because their prerequisite did not finish successfully. Existing edits remain in the shared worktree.",
      promptSnippet: "Cancel one build child and its dependent slices",
      promptGuidelines: ["Cancel children whose work is no longer needed, then inspect existing edits and reports; cancellation does not roll back shared files."],
      parameters: Type.Object({ id: Type.String() }, { additionalProperties: false }),
      async execute(_id, params: { id: string }) {
        const cancelled = runtime.cancel(params.id);
        return { content: [{ type: "text", text: cancelled ? `Cancelled ${params.id}; existing edits remain.` : `Child ${params.id} is not live; nothing cancelled.` }], details: { cancelled } };
      },
    },
  ];
}
