// The Pi-native reviewer role (T-252 reviewer-role): upstream read, grep, find, ls and bash, with
// no edit or write tool, plus corral_ask, corral_withdraw, corral_evidence, corral_artifact and
// corral_verdict, and the reviewer contract from the installed assets. Each review child starts
// its current request after hello, even in a retained conversation. An observed attempt is not
// submitted again when the same child recovers in another host generation.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { CorralOp, CorralToolSpec, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { artifactTool, askTool, evidenceTool, withdrawTool } from "../common/tools.ts";
import { verdictTool } from "./tools.ts";

export const CONTRACT_ASSET = "assets/reviewer-contract.md";

const UPSTREAM: readonly UpstreamTool[] = ["read", "grep", "find", "ls", "bash"];
const TOOLS: readonly CorralToolSpec[] = [askTool, withdrawTool, evidenceTool, artifactTool, verdictTool];
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", ...TOOLS.map((tool) => tool.op)];

/** The current review request: its prompt and how the commands map. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete review prompt for corral task `
    + `${identity.task ?? ""}, run ${identity.run ?? ""} (review ${identity.review ?? ""}). Do not edit files. `
    + "Where it tells you to finish with `corral verdict`, call corral_verdict with the same verdict and your "
    + "notes instead; corral already knows the attempt. Attach evidence with corral_artifact or corral_evidence "
    + "and ask with corral_ask rather than in the pane. Its Gate contract (review, authoritative) section is the "
    + "tier you judge the candidate against.";
}

export function reviewerRole(root: string): RoleModule {
  return {
    role: "reviewer",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools() {
      return TOOLS;
    },
    contract(context: RoleContext) {
      return renderContract(readAsset(root, CONTRACT_ASSET), context.identity);
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const { store, task, run, review, callback } = context.identity;
      const marker = `[corral review request ${JSON.stringify([store, task, run, review, callback])}]`;
      let submitted = false;
      pi.on("session_start", (event, ctx) => {
        const text = kickoff(context);
        if (event.reason !== "startup" || submitted || text === undefined) return;
        const matches = (content: string) => content.includes(marker)
          // A recovering child may have observed its request before attempt tokens existed.
          || (context.identity.generation > 1 && content === text);
        const observed = ctx.sessionManager.getEntries().some((entry) =>
          entry.type === "message" && entry.message.role === "user"
          && (typeof entry.message.content === "string"
            ? matches(entry.message.content)
            : entry.message.content.some((part) => part.type === "text" && matches(part.text))));
        submitted = true;
        if (!observed) pi.sendUserMessage(`${text}\n\n${marker}`);
      });
    },
  };
}

export default reviewerRole(assetRoot(import.meta.url));
