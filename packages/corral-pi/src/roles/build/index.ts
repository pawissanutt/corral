import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { askUserContract } from "../common/ask-user.ts";
import { calibrationReader } from "../common/family.ts";
import { requestContract, sessionTodoTool } from "../common/todo.ts";
import { receiptLookupTool } from "../common/tools.ts";
import type { CorralOp, RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { buildShellRefusal, installBuildGuard } from "./guard.ts";

export const CONTRACT_ASSET = "assets/build-contract.md";
export const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "grep", "find", "ls", "bash"];
const LOOKUP = receiptLookupTool();
const OPS: readonly CorralOp[] = ["hello", "event", "receipt_lookup", "todo"];

function sessionFacts(context: RoleContext): string {
  if (!context.build) return "## This build session\n\nNo build session is bound. Writes are refused until the host binds a build worktree.";
  const { session, settings } = context.build;
  const profiles = settings.profiles.map((profile) =>
    `- ${profile.name}: ${profile.model}${profile.description ? `; ${profile.description}` : ""}`);
  return [
    "## This build session",
    "",
    `Worktree: ${session.worktree}`,
    `Branch: ${session.branch ?? "detached HEAD"}`,
    `Base commit: ${session.base ?? "none (unborn repository)"}`,
    `Owner's checkout: ${session.here ? "yes" : "no (a separate session worktree)"}`,
    `Exit policy: ${settings.exitPolicy}`,
    `Maximum parallel children: ${settings.maxParallel}`,
    "",
    "Child profiles (name: model; description when configured):",
    ...profiles,
  ].join("\n");
}

export function buildRole(root: string): RoleModule {
  return {
    role: "build",
    upstreamTools: UPSTREAM,
    corralOps: OPS,
    tools: (context) => context.todo === true ? [LOOKUP, sessionTodoTool] : [LOOKUP],
    contract: (context) => `${renderContract(readAsset(root, CONTRACT_ASSET), context.identity)}\n\n`
      + requestContract(root, context.todo) + "\n\n" + askUserContract(context) + "\n\n" + sessionFacts(context),
    calibration: calibrationReader(root, "build"),
    shellRefusal: buildShellRefusal,
    extend: installBuildGuard,
  };
}

export default buildRole(assetRoot(import.meta.url));
