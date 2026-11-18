// The legacy PM role keeps its prompt and skill until the chat roles merge.
import { assetPath, assetRoot, readAsset, renderContract } from "../common/assets.ts";
import type { RoleModule } from "../types.ts";
import { operationalContext } from "./context.ts";
import { requestContract, sessionTodoTool } from "../common/todo.ts";
import { askUserContract } from "../common/ask-user.ts";
import { agentCommunicationContract } from "../common/agent-communication.ts";
import { calibrationReader } from "../common/family.ts";

export const CONTEXT_ASSET = "assets/pm-context.md";
export const SKILL_ASSET = "src/roles/pm/skills/corral-pm/SKILL.md";

export function pmRole(root: string): RoleModule {
  return {
    role: "pm",
    // Owner direction 2026-10-30: the PM can read, edit and run commands like an assistant.
    upstreamTools: ["read", "edit", "write", "grep", "find", "ls", "bash"],
    tools: (context) => context.todo === true ? [sessionTodoTool] : [],
    // HostSession captures this once per launch; reload/compaction reuse the same contract.
    contract: (context) => `${renderContract(readAsset(root, CONTEXT_ASSET), context.identity)}\n`
      + agentCommunicationContract() + "\n\n"
      + requestContract(root, context.todo) + "\n\n" + askUserContract(context) + operationalContext(context.promptFile),
    calibration: calibrationReader(root, "pm"),
    extend(pi) {
      const skill = assetPath(root, SKILL_ASSET);
      pi.on("resources_discover", () => ({ skillPaths: [skill] }));
    },
  };
}

export default pmRole(assetRoot(import.meta.url));
