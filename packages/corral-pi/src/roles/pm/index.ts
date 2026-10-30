// PM authority lives in Rust; this role supplies only the project-scoped typed loadout.
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import type { CorralOp, RoleModule } from "../types.ts";
import { pmTools } from "./tools.ts";
import { todoTool } from "../common/todo.ts";

export const CONTEXT_ASSET = "assets/pm-context.md";
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", ...pmTools.map((tool) => tool.op), "todo"];

export function pmRole(root: string): RoleModule {
  return {
    role: "pm",
    upstreamTools: [],
    corralOps: OPS,
    tools: (context) => context.todo === true ? [...pmTools, todoTool] : pmTools,
    contract: (context) => renderContract(readAsset(root, CONTEXT_ASSET), context.identity),
  };
}

export default pmRole(assetRoot(import.meta.url));
