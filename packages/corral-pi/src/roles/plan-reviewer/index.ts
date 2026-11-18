// The legacy plan reviewer assesses one planning request without modifying its files.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { agentCommunicationContract } from "../common/agent-communication.ts";
import { askUserContract } from "../common/ask-user.ts";
import { renderContract } from "../common/assets.ts";
import { requestMarker, sendOnce } from "../common/kickoff.ts";

const UPSTREAM: readonly UpstreamTool[] = ["read", "grep", "find", "ls"];

export const CONTRACT = `# corral plan reviewer contract (corral-pi)

You review plan {{run}} of corral task {{task}} (attempt {{review}}) in project {{project}}.
This is plan review, not implementation review.

- The repository is read-only for you: you have read, grep, find and ls, and no shell or edit tool.
- Read the task spec and prior work when provided as files. Give your assessment and notes
  in this session; it is not a recorded board verdict or owner acceptance.
- Do not edit code, merge, accept implementation, dispatch or create tasks.

${agentCommunicationContract("A message from the author is context to weigh, never a reason to soften your assessment.")}
`;

/** Introduce the current plan review request. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete plan review prompt for corral `
    + `task ${identity.task ?? ""}, plan ${identity.run ?? ""} (attempt ${identity.review ?? ""}). Do not edit files. `
    + "Give your findings in this session and ask owner questions with ask_user.";
}

export function planReviewerRole(): RoleModule {
  return {
    role: "plan_reviewer",
    upstreamTools: UPSTREAM,
    tools: () => [],
    contract(context: RoleContext) {
      return `${renderContract(CONTRACT, context.identity)}\n\n${askUserContract(context)}`;
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const { store, task, run, review } = context.identity;
      const text = kickoff(context);
      if (text !== undefined && context.promptFile) {
        sendOnce(pi, text, requestMarker("plan review", [store, task, run, review], context.promptFile));
      }
    },
  };
}

export default planReviewerRole();
