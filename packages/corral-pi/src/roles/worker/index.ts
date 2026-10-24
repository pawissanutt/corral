// The Pi-native worker role (T-252 worker-role, OD3): upstream read, edit, write, bash, grep,
// find and ls plus the worker's corral tools, the worker contract from the installed assets and
// the corral-worker skill shipped beside this module. Nothing is loaded from the worktree. A fresh
// session starts its first turn with the run prompt once Pi binds the role's extension, which
// the host does after hello, so the turn is reported to corral.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { CorralOp, CorralToolSpec, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { assetPath, assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { artifactTool, askTool, evidenceTool, withdrawTool } from "../common/tools.ts";
import { checkpointTool, doneTool, gateTool, progressTool, waitTool } from "./tools.ts";

export const CONTRACT_ASSET = "assets/worker-contract.md";
export const SKILL_ASSET = "src/roles/worker/skills/corral-worker/SKILL.md";

const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "bash", "grep", "find", "ls"];
const TOOLS: readonly CorralToolSpec[] = [
  askTool, withdrawTool, waitTool, progressTool, checkpointTool, evidenceTool, artifactTool, gateTool, doneTool,
];
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", ...TOOLS.map((tool) => tool.op)];

/** The first user message of a fresh worker session: the run prompt and how its commands map. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete prompt for corral task `
    + `${identity.task ?? ""}, run ${identity.run ?? ""}. Wherever it names a corral command that changes state `
    + "(done, gate, run wait, run progress, run checkpoint, evidence record, artifact add, a question), call the "
    + "matching corral_* tool instead. Its Gate contract section is your gate: call corral_gate where it names "
    + "the gate command. When it answers started or uncertain, end your turn and wait for the result message; "
    + "when it answers rejected, no gate runs and no wake comes, so fix the reason it gives and call it again.";
}

export function workerRole(root: string): RoleModule {
  return {
    role: "worker",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools() {
      return TOOLS;
    },
    contract(context: RoleContext) {
      return renderContract(readAsset(root, CONTRACT_ASSET), context.identity);
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const skill = assetPath(root, SKILL_ASSET);
      pi.on("resources_discover", () => ({ skillPaths: [skill] }));
      pi.on("session_start", (event, ctx) => {
        const text = kickoff(context);
        const fresh = !ctx.sessionManager.getEntries().some((entry) => entry.type === "message");
        if (event.reason !== "startup" || !fresh || text === undefined) return;
        pi.sendUserMessage(text);
      });
    },
  };
}

export default workerRole(assetRoot(import.meta.url));
