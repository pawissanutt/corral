// PM authority lives in Rust; this role supplies only the project-scoped typed loadout.
import { assetPath, assetRoot, readAsset, renderContract } from "../common/assets.ts";
import type { CorralOp, RoleModule } from "../types.ts";
import { pmTools } from "./tools.ts";
import { operationalContext } from "./context.ts";
import { requestContract, sessionTodoTool } from "../common/todo.ts";

export const CONTEXT_ASSET = "assets/pm-context.md";
export const SKILL_ASSET = "src/roles/pm/skills/corral-pm/SKILL.md";
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", ...pmTools.map((tool) => tool.op), "todo"];

export function pmRole(root: string): RoleModule {
  return {
    role: "pm",
    // Owner direction 2026-10-30: the PM can read, edit and run commands like an assistant.
    upstreamTools: ["read", "edit", "write", "grep", "find", "ls", "bash"],
    corralOps: OPS,
    tools: (context) => context.todo === true ? [...pmTools, sessionTodoTool] : pmTools,
    contract: (context) => `${renderContract(readAsset(root, CONTEXT_ASSET), context.identity)}\n\n`
      + requestContract(root, context.todo) + operationalContext(context.promptFile),
    extend(pi) {
      const skill = assetPath(root, SKILL_ASSET);
      pi.on("resources_discover", () => ({ skillPaths: [skill] }));
    },
  };
}

export default pmRole(assetRoot(import.meta.url));
