// Assistant authority lives in Rust: this role supplies the workspace-scoped loadout and contract.
// Reads and filing go through the corral CLI from bash; edit and write reach only the session's
// scratch worktrees, session artifact directory and temp directory (paths.ts). Owner-level
// commands the owner asks for go through corral_owner_action (owner-action.ts).
import { closeSync, openSync, readSync } from "node:fs";
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { todoTool } from "../common/todo.ts";
import type { CorralOp, RoleModule, UpstreamTool } from "../types.ts";
import { assistantShellRefusal } from "./guard.ts";
import { ownerActionTool } from "./owner-action.ts";
import { installWriteGuard } from "./paths.ts";

export const CONTRACT_ASSET = "assets/assistant-contract.md";
export const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "grep", "find", "ls", "bash"];
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", "todo", "owner_action"];

export function assistantRole(root: string): RoleModule {
  return {
    role: "assistant",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools: (context) => context.todo === true ? [ownerActionTool, todoTool] : [ownerActionTool],
    contract: (context) => {
      const contract = renderContract(readAsset(root, CONTRACT_ASSET), context.identity);
      if (!context.promptFile) return contract;
      // The Rust launcher writes this read-only task brief, not a worker prompt. Bound the
      // read again here so a replaced file cannot grow the model's system context.
      const fd = openSync(context.promptFile, "r");
      try {
        const bytes = Buffer.alloc(24 * 1024 + 1);
        const n = readSync(fd, bytes);
        if (n > 24 * 1024) throw new Error("task brief exceeds 24 KiB");
        return `${contract}\n\n${bytes.subarray(0, n).toString("utf8")}`;
      } finally {
        closeSync(fd);
      }
    },
    shellRefusal: assistantShellRefusal,
    extend: installWriteGuard,
  };
}

export default assistantRole(assetRoot(import.meta.url));
