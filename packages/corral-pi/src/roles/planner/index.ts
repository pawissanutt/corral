// The legacy planner reviews repository context without a write or shell tool.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { agentCommunicationContract } from "../common/agent-communication.ts";
import { askUserContract } from "../common/ask-user.ts";
import { renderContract } from "../common/assets.ts";
import { requestMarker, sendOnce } from "../common/kickoff.ts";

const UPSTREAM: readonly UpstreamTool[] = ["read", "grep", "find", "ls"];

export const CONTRACT = `# corral planner contract (corral-pi)

You are the planner of corral task {{task}}, design run {{run}}, in project {{project}}.
You draft one plan; you do not implement it, review it, approve it or dispatch work.

- The repository is read-only for you: you have read, grep, find and ls, and no shell or edit tool.
- Read the task spec and prior work when provided as files. Present your draft as Markdown
  in the session; this role does not write or submit a board plan.
- Review and owner acceptance are separate steps that are never yours.

${agentCommunicationContract()}
`;

/** Introduce the current planning request. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete planning prompt for corral `
    + `task ${identity.task ?? ""}, design run ${identity.run ?? ""}. The repository is read-only and you have `
    + "no shell. Present the plan in this session and ask owner questions with ask_user.";
}

export function plannerRole(): RoleModule {
  return {
    role: "planner",
    upstreamTools: UPSTREAM,
    tools: () => [],
    contract(context: RoleContext) {
      return `${renderContract(CONTRACT, context.identity)}\n\n${askUserContract(context)}`;
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const { store, task, run } = context.identity;
      const text = kickoff(context);
      if (text !== undefined && context.promptFile) {
        sendOnce(pi, text, requestMarker("planning", [store, task, run], context.promptFile));
      }
    },
  };
}

export default plannerRole();
