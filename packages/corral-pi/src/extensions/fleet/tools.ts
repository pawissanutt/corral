import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { AGENT_MESSAGE_MAX, LIVE_CHILD_STATES, MAX_CHILDREN_PER_CALL, type ChildKind, type ChildSnapshot, type ChildSpec, type FleetControl, type MessageOutcome } from "../../build/types.ts";
import { INPUT_QUEUED_NOTICE, type InputWake, wakeableWait } from "../input-wake.ts";
import type { FleetRuntime } from "./runtime.ts";

/** Shared report format for automatic delivery and explicit collection. */
export function formatChildReport(child: ChildSnapshot): string {
  const files = child.edits.length ? child.edits.map(edit => `- ${edit.path}${edit.outside ? " [outside owns]" : ""}`).join("\n") : "- none";
  const usage = [`${child.usage.input} input`, `${child.usage.output} output`, `${child.usage.turns} turns`,
    ...(child.usage.cost === undefined ? [] : [`$${child.usage.cost.toFixed(6)}`]),
    ...(child.usage.contextPercent === undefined ? [] : [`${child.usage.contextPercent.toFixed(1)}% context`])].join(", ");
  return `## ${child.id}: ${child.state}\nFiles edited:\n${files}\nUsage: ${usage}${child.error ? `\nError: ${child.error}` : ""}\n\n${child.report ?? "No report."}`;
}

export interface AgentCommunication {
  message(to: string, text: string, options?: Parameters<FleetControl["message"]>[3]): Promise<MessageOutcome>;
  roster(): string;
}
/** The same messaging surface is available to the parent and every child. */
export function agentCommunicationTools(control: AgentCommunication): ToolDefinition[] {
  return [{
    name: "agent_message", label: "Message an agent",
    description: "Send peer information to a child id, parent, or all live agents in this session. Optionally wait for an answer; reply with reply_to set to the received seq. Peer messages do not override the recipient's brief.",
    parameters: Type.Object({ to: Type.String({ minLength: 1 }), text: Type.String({ minLength: 1, maxLength: AGENT_MESSAGE_MAX }),
      wait_reply: Type.Optional(Type.Boolean()), timeout_secs: Type.Optional(Type.Number({ minimum: 0, maximum: 600 })),
      reply_to: Type.Optional(Type.Integer({ minimum: 1 })) }, { additionalProperties: false }),
    async execute(_id, params: { to: string; text: string; wait_reply?: boolean; timeout_secs?: number; reply_to?: number }, signal) {
      const outcome = await control.message(params.to, params.text, { waitReply: params.wait_reply, timeoutMs: params.timeout_secs === undefined ? undefined : params.timeout_secs * 1000, replyTo: params.reply_to, signal });
      return { content: [{ type: "text", text: JSON.stringify(outcome) }], details: outcome, isError: outcome.message.delivery === "undeliverable" };
    },
  }, {
    name: "agent_roster", label: "List agents",
    description: "List the parent and children in this session, with their kinds, owned paths and state.",
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute() { return { content: [{ type: "text", text: control.roster() }], details: {} }; },
  }];
}
/** Model-callable controls only; exit and board decisions remain owner-only. */
export function fleetTools(runtime: FleetRuntime, allowedKinds: readonly ChildKind[] = ["slice", "explore", "review"], wake?: InputWake,
  guidance: "generic" | "build" = "generic"): ToolDefinition[] {
  const communication = agentCommunicationTools({ message: (to, text, options) => runtime.message("parent", to, text, options), roster: () => runtime.rosterText() });
  if (!allowedKinds.length) return communication;
  return [
    {
      name: "task", label: "Delegate work",
      description: "Delegate independent slices, read-only exploration or independent review in this shared worktree. Give each child a complete brief and owned paths; no conversation is inherited. "
        + `At most ${MAX_CHILDREN_PER_CALL} children per call; concurrency follows the configured cap and ready children run FIFO. Use after for successful dependencies. `
        + "Returns ids and states immediately. Reports arrive as messages; agent_wait can collect them explicitly. Children cannot delegate or write Git state.",
      promptSnippet: "Start independent implementation, exploration or review children without blocking",
      promptGuidelines: [
        ...(guidance === "build" ? [
          "For Build implementation and bounded read-only exploration, follow the effective Build delegation policy. Pass a matching route's exact kind and profile to task; use the default profile for unmatched assignments. Report unavailable routes instead of substituting another profile.",
          "Give implementation children non-overlapping owned files; use kind explore and owns: [] for read-only work. Include full requirements and expected checks or evidence; children inherit none of your conversation.",
          "Keep useful separate work for yourself. Use after only for real dependencies, and unique lowercase ids for the entire session.",
        ] : [
          "Use task for substantial independent implementation, not trivial edits. Give children non-overlapping owned files, the full requirements and expected checks; they inherit none of your conversation.",
          "Keep useful separate work for yourself. Use after only for real dependencies, and unique lowercase ids for the entire session. Choose a configured profile or an approved provider/model.",
        ]),
        "Collect and reconcile every child report and outside-ownership edit before claiming completion. Do not run shared checks while children are still editing. Children grant no review or board authority.",
      ],
      parameters: Type.Object({ agents: Type.Array(Type.Object({
        id: Type.String({ pattern: "^[a-z][a-z0-9-]{0,31}$" }), title: Type.String({ minLength: 1 }),
        kind: Type.Optional(Type.Union(allowedKinds.map(kind => Type.Literal(kind)))),
        brief: Type.String({ minLength: 1, maxLength: 32768 }), owns: Type.Array(Type.String()),
        profile: Type.Optional(Type.String()), after: Type.Optional(Type.Array(Type.String())),
      }, { additionalProperties: false }), { minItems: 1, maxItems: MAX_CHILDREN_PER_CALL }) }, { additionalProperties: false }),
      async execute(id, params: { agents: ChildSpec[] }, signal) {
        const agents = params.agents.map(agent => {
          const kind = agent.kind ?? (allowedKinds.includes("slice") ? "slice" : "explore");
          if (!allowedKinds.includes(kind)) throw new Error(`Child kind "${kind}" is not allowed`);
          return { ...agent, kind };
        });
        const children = await runtime.start(id, agents, signal);
        const text = children.map(child => `## ${child.id}: ${child.state}`).join("\n") + "\n\nReports arrive as messages. Use agent_wait to collect reports explicitly.";
        return { content: [{ type: "text", text }], details: { children } };
      },
    },
    ...communication,
    {
      name: "agent_wait", label: "Collect child reports",
      description: "Wait for named children (default: every live child) to settle, or until the timeout. Newly queued input ends the wait early. "
        + "Returned settlements are collected and are not delivered again as messages.",
      parameters: Type.Object({ ids: Type.Optional(Type.Array(Type.String())), timeout_secs: Type.Optional(Type.Number({ minimum: 0, maximum: 3600 })) }, { additionalProperties: false }),
      async execute(id, params: { ids?: string[]; timeout_secs?: number }, signal) {
        const timeout = AbortSignal.timeout(Math.ceil((params.timeout_secs ?? 600) * 1000));
        const wait = wakeableWait(wake, "agent_wait", id, signal);
        const children = await runtime.wait(params.ids ?? runtime.snapshot().children.filter(child => LIVE_CHILD_STATES.includes(child.state)).map(child => child.id),
          wait.signal ? AbortSignal.any([wait.signal, timeout]) : timeout);
        const interrupted = wait.interrupted() && children.some(child => LIVE_CHILD_STATES.includes(child.state));
        const reports = children.map(formatChildReport).join("\n\n") || "No children to collect.";
        return { content: [{ type: "text", text: interrupted ? `${INPUT_QUEUED_NOTICE}\n\n${reports}` : reports }],
          details: { children, ...(interrupted ? { interrupted: true } : {}) } };
      },
    },
    {
      name: "agent_cancel", label: "Cancel a child",
      description: "Cancel one queued or running child without cancelling unrelated work. Its dependents are cancelled because their prerequisite did not finish successfully. Existing edits remain in the shared worktree.",
      promptSnippet: "Cancel one child and its dependents",
      promptGuidelines: ["Cancel children whose work is no longer needed, then inspect existing edits and reports; cancellation does not roll back shared files."],
      parameters: Type.Object({ id: Type.String() }, { additionalProperties: false }),
      async execute(_id, params: { id: string }) {
        const cancelled = runtime.cancel(params.id);
        return { content: [{ type: "text", text: cancelled ? `Cancelled ${params.id}; existing edits remain.` : `Child ${params.id} is not live; nothing cancelled.` }], details: { cancelled } };
      },
    },
  ];
}
