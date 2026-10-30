// Assistant authority lives in Rust: this role supplies the workspace-scoped loadout and contract.
// Reads and filing go through the corral CLI from bash; edit and write reach only the session's
// scratch worktrees, session artifact directory and temp directory (paths.ts).
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { todoTool } from "../common/todo.ts";
import type { CorralOp, RoleModule, UpstreamTool } from "../types.ts";
import { assistantShellRefusal } from "./guard.ts";
import { installWriteGuard } from "./paths.ts";

export const CONTRACT_ASSET = "assets/assistant-contract.md";
export const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "grep", "find", "ls", "bash"];
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", "todo"];

export function assistantRole(root: string): RoleModule {
  return {
    role: "assistant",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools: (context) => context.todo === true ? [todoTool] : [],
    contract: (context) => renderContract(readAsset(root, CONTRACT_ASSET), context.identity),
    shellRefusal: assistantShellRefusal,
    extend: installWriteGuard,
  };
}

export default assistantRole(assetRoot(import.meta.url));
