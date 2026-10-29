// The Pi-native plan reviewer role (T-460 plan-roles): upstream read, grep, find and ls, with no
// shell, edit or write tool, plus corral_ask, corral_withdraw, corral_plan_write (its notes
// report) and corral_plan_verdict. It reviews one bound plan attempt and never accepts a plan.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { CorralOp, CorralToolSpec, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { askTool, withdrawTool } from "../common/tools.ts";
import { renderContract } from "../common/assets.ts";
import { requestMarker, sendOnce } from "../planner/kickoff.ts";
import { planWriteTool } from "../planner/tools.ts";
import { planVerdictTool } from "./tools.ts";

const UPSTREAM: readonly UpstreamTool[] = ["read", "grep", "find", "ls"];
const TOOLS: readonly CorralToolSpec[] = [askTool, withdrawTool, planWriteTool("notes"), planVerdictTool];
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", ...TOOLS.map((tool) => tool.op)];

export const CONTRACT = `# corral plan reviewer contract (Pi-native)

You review plan {{run}} of corral task {{task}} (attempt {{review}}) in project {{project}}.
This is plan review, not implementation review.

- The repository is read-only for you: you have read, grep, find and ls, and no shell or edit tool.
- corral_plan_verdict records approve, changes or escalate with your notes for this attempt only.
  An approval is never owner acceptance; owner decisions stay with the owner.
- corral_plan_write drafts long notes into your notes report; corral chooses that path.
- corral_ask records a question for the PM or the owner; withdraw one you no longer need.
- Do not edit code, merge, accept implementation, dispatch or create tasks.
`;

/** The current plan review request: its prompt and how its commands map to tools. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete plan review prompt for corral `
    + `task ${identity.task ?? ""}, plan ${identity.run ?? ""} (attempt ${identity.review ?? ""}). Do not edit files. `
    + "Where it tells you to write notes to a file and finish with `corral plan verdict`, call corral_plan_verdict "
    + "with the same verdict and your notes instead; corral already knows the plan and attempt and records the "
    + "notes on the review. To keep the notes file the prompt names, or for notes over 16 KiB, write them first with corral_plan_write and call corral_plan_verdict "
    + "without notes. Ask with corral_ask rather than in the pane.";
}

export function planReviewerRole(): RoleModule {
  return {
    role: "plan_reviewer",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools() {
      return TOOLS;
    },
    contract(context: RoleContext) {
      return renderContract(CONTRACT, context.identity);
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const { store, task, run, review } = context.identity;
      if (!context.promptFile) return;
      sendOnce(pi, kickoff(context), requestMarker("plan review", [store, task, run, review], context.promptFile));
    },
  };
}

export default planReviewerRole();
