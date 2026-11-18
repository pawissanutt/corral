// The assistant works with ordinary file and shell tools and keeps a host-local request todo.
import { closeSync, openSync, readSync } from "node:fs";
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { requestContract, sessionTodoTool } from "../common/todo.ts";
import { askUserContract } from "../common/ask-user.ts";
import { calibrationReader } from "../common/family.ts";
import type { RoleModule, UpstreamTool } from "../types.ts";
import { assistantShellRefusal } from "./guard.ts";
export const CONTRACT_ASSET = "assets/assistant-contract.md";
export const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "grep", "find", "ls", "bash"];

export function assistantRole(root: string): RoleModule {
  return {
    role: "assistant",
    upstreamTools: UPSTREAM,
    tools: (context) => context.todo === true ? [sessionTodoTool] : [],
    contract: (context) => {
      const contract = `${renderContract(readAsset(root, CONTRACT_ASSET), context.identity)}\n\n`
        + requestContract(root, context.todo) + "\n\n" + askUserContract(context);
      if (!context.promptFile) return contract;
      // A task brief is read-only context, not a worker prompt. Bound its system-prompt size.
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
    calibration: calibrationReader(root, "assistant"),
    shellRefusal: assistantShellRefusal,
  };
}

export default assistantRole(assetRoot(import.meta.url));
