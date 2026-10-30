// The Pi-native planner role (T-460 plan-roles): upstream read, grep, find and ls, with no shell,
// edit or write tool, plus corral_ask, corral_withdraw, corral_plan_write and corral_plan_submit.
// The source tree stays read-only; corral writes only this run's plan file. Submitting is the
// planner's one completion step; it never reviews, approves or dispatches its plan.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { CorralOp, CorralToolSpec, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { askTool, withdrawTool } from "../common/tools.ts";
import { renderContract } from "../common/assets.ts";
import { requestMarker, sendOnce } from "./kickoff.ts";
import { CHUNK_MAX_BYTES, planSubmitTool, planWriteTool } from "./tools.ts";

const UPSTREAM: readonly UpstreamTool[] = ["read", "grep", "find", "ls"];
const TOOLS: readonly CorralToolSpec[] = [askTool, withdrawTool, planWriteTool("plan"), planSubmitTool];
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", ...TOOLS.map((tool) => tool.op)];

export const CONTRACT = `# corral planner contract (Pi-native)

You are the planner of corral task {{task}}, design run {{run}}, in project {{project}}.
You write one plan; you do not implement it, review it, approve it or dispatch work.

- The repository is read-only for you: you have read, grep, find and ls, and no shell or edit tool.
- corral_plan_write writes the plan file your prompt names; corral chooses that path. Write in
  chunks of at most ${CHUNK_MAX_BYTES / 1024} KiB, each continuing at the previous receipt's \`bytes\`.
- corral_plan_submit submits it, as \`corral plan submit\` would. Fix and resubmit a refused plan.
- corral_ask records a question for the PM or the owner; withdraw one you no longer need.
- Review and owner acceptance are separate steps that are never yours.
`;

/** The current planning request: its prompt and how its commands map to tools. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete planning prompt for corral `
    + `task ${identity.task ?? ""}, design run ${identity.run ?? ""}. The repository is read-only and you have `
    + "no shell. Write the plan with corral_plan_write instead of writing the file the prompt names: it writes "
    + "exactly that file. Where the prompt tells you to finish with `corral plan submit`, call corral_plan_submit "
    + "instead. Ask with corral_ask rather than in the pane.";
}

export function plannerRole(): RoleModule {
  return {
    role: "planner",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools() {
      return TOOLS;
    },
    contract(context: RoleContext) {
      return renderContract(CONTRACT, context.identity);
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const { store, task, run } = context.identity;
      if (!context.promptFile) return;
      sendOnce(pi, kickoff(context), requestMarker("planning", [store, task, run], context.promptFile));
    },
  };
}

export default plannerRole();
