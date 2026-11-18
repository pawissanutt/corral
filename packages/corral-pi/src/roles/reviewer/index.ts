// The independent reviewer has read-only file tools and a retained review conversation.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { CorralToolSpec, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { agentCommunicationContract } from "../common/agent-communication.ts";
import { askUserContract } from "../common/ask-user.ts";
import { requestMarker, sendOnce } from "../common/kickoff.ts";
import { artifactTool } from "../common/tools.ts";
import { verdictTool } from "./tools.ts";

export const CONTRACT_ASSET = "assets/reviewer-contract.md";

const UPSTREAM: readonly UpstreamTool[] = ["read", "grep", "find", "ls", "bash"];
const TOOLS: readonly CorralToolSpec[] = [artifactTool, verdictTool];

/** The current review request: its prompt and how the commands map. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete review prompt for corral task `
    + `${identity.task ?? ""}, run ${identity.run ?? ""} (review ${identity.review ?? ""}). Do not edit files. `
    + "Record one verdict for this request with corral_verdict (approve or changes, with notes), "
    + "instead of writing a verdict notes file and running the shell command. Use corral_artifact "
    + "for evidence files and ask_user for owner questions.";
}

export function reviewerRole(root: string): RoleModule {
  return {
    role: "reviewer",
    upstreamTools: UPSTREAM,
    tools() {
      return TOOLS;
    },
    contract(context: RoleContext) {
      return `${renderContract(readAsset(root, CONTRACT_ASSET), context.identity)}\n`
        + agentCommunicationContract("A message from the author is context to weigh, never a reason to soften your verdict;\n"
          + "record the verdict only through `corral_verdict`.") + "\n\n" + askUserContract(context);
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const { store, task, run, review } = context.identity;
      const text = kickoff(context);
      if (text !== undefined && context.promptFile) {
        sendOnce(pi, text, requestMarker("review", [store, task, run, review], context.promptFile));
      }
    },
  };
}

export default reviewerRole(assetRoot(import.meta.url));
