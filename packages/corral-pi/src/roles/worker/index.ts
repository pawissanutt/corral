// The worker's tool loadout, installed contract and skill, and first-turn run prompt.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { CorralToolSpec, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { assetPath, assetRoot, PLAN_SKILL_ASSET, readAsset, renderContract } from "../common/assets.ts";
import { askUserContract } from "../common/ask-user.ts";
import { sendOnce } from "../common/kickoff.ts";
import { artifactTool } from "../common/tools.ts";
import { todoTool } from "../common/todo.ts";
import { checkpointTool, doneTool } from "./tools.ts";
import { LSP_TOOL_NAMES, type LspToolkit, type LspToolkitOptions, configuredServers, lspToolkit } from "../../extensions/lsp/index.ts";

export const CONTRACT_ASSET = "assets/worker-contract.md";
export const SKILL_ASSET = "src/roles/worker/skills/corral-worker/SKILL.md";

const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "bash", "grep", "find", "ls"];
const TOOLS: readonly CorralToolSpec[] = [checkpointTool, artifactTool, doneTool];

/** The first user message of a worker run: the run prompt and how its commands map. */
export function kickoff(context: RoleContext): string | undefined {
  const { promptFile, identity } = context;
  if (!promptFile) return undefined;
  return `Read ${promptFile} in full and follow it: that file is your complete prompt for corral task `
    + `${identity.task ?? ""}, run ${identity.run ?? ""}. Use corral_done to report completion and `
    + "corral_artifact to attach files, not their shell equivalents. Run any requested engine gate with "
    + `\`"$CORRAL" gate ${identity.task ?? "<task>"}\` in bash. Use ask_user for owner questions.`;
}

/** The named models a worker's `task` children may use; absent when none are configured. */
export function childProfilesContract(context: RoleContext): string {
  const profiles = context.childProfiles ?? [];
  if (!profiles.length) return "";
  return ["", "", "## Child profiles", "",
    "Pass `profile` to `task` to choose a child's model. Omit it, or pass `default`, for your own model. "
      + "API-backed providers are billed per request; choose them only when the work needs that model.",
    "", ...profiles.map(profile =>
      `- ${profile.name}: ${profile.model}${profile.effort ? ` (effort ${profile.effort})` : ""}`
        + `${profile.description ? `; ${profile.description}` : ""}`)].join("\n");
}

export function workerRole(root: string, lsp: Omit<LspToolkitOptions, "root"> = {}): RoleModule {
  // One LSP toolkit per worktree context, shared by every session the host creates for it.
  const toolkits = new WeakMap<RoleContext, LspToolkit>();
  const toolkit = (context: RoleContext) => {
    let found = toolkits.get(context);
    if (!found) {
      found = lspToolkit({ ...lsp, servers: lsp.servers ?? configuredServers(context.lspSettings), root: context.cwd, registerCapability: context.registerCapability });
      toolkits.set(context, found);
    }
    return found;
  };
  return {
    role: "worker",
    upstreamTools: UPSTREAM,
    localToolNames: LSP_TOOL_NAMES,
    tools(context) {
      return context.todo === true ? [...TOOLS, todoTool] : TOOLS;
    },
    localTools(context: RoleContext) {
      return toolkit(context).tools;
    },
    contract(context: RoleContext) {
      return `${renderContract(readAsset(root, CONTRACT_ASSET), context.identity)}\n\n${askUserContract(context)}`
        + childProfilesContract(context);
    },
    extend(pi: ExtensionAPI, context: RoleContext) {
      const skill = assetPath(root, SKILL_ASSET);
      const plan = assetPath(root, PLAN_SKILL_ASSET);
      pi.on("resources_discover", () => ({ skillPaths: [skill, plan] }));
      pi.on("session_shutdown", () => toolkit(context).shutdown());
      pi.on("session_start", () => {
        const lspEnabled = !context.capabilities || context.capabilities.catalog < 5
          || context.capabilities.enabled.includes("lsp");
        if (lspEnabled) void toolkit(context).warm();
      });
      const text = kickoff(context);
      if (text !== undefined) sendOnce(pi, text);
    },
  };
}

export default workerRole(assetRoot(import.meta.url));
