// The Pi-native worker role (T-252 worker-role, OD3): upstream read, edit, write, bash, grep,
// find and ls plus the worker's corral tools, the worker contract from the installed assets and
// the corral-worker skill shipped beside this module. Nothing is loaded from the worktree.
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
    extend(pi: ExtensionAPI) {
      const skill = assetPath(root, SKILL_ASSET);
      pi.on("resources_discover", () => ({ skillPaths: [skill] }));
    },
  };
}

export default workerRole(assetRoot(import.meta.url));
