import type { BuildSettings } from "../../build/types.ts";
import { assetRoot, readAsset, renderContract } from "../common/assets.ts";
import { askUserContract } from "../common/ask-user.ts";
import { calibrationReader } from "../common/family.ts";
import { planPrompts } from "../common/plan-mode.ts";
import { requestContract, sessionTodoTool } from "../common/todo.ts";
import type { RoleContext, RoleModule, UpstreamTool } from "../types.ts";
import { buildShellRefusal, installBuildGuard } from "./guard.ts";

export const CONTRACT_ASSET = "assets/build-contract.md";
export const UPSTREAM: readonly UpstreamTool[] = ["read", "edit", "write", "grep", "find", "ls", "bash"];

function delegationFacts(settings: BuildSettings): string[] {
  const mode = settings.delegation?.mode ?? "standard";
  const routes = settings.delegation?.routes ?? [];
  return [
    "## Effective Build delegation policy",
    "",
    `Mode: ${mode}`,
    mode === "proactive"
      ? "Start substantial independent slices early once interfaces and disjoint owned paths are clear. Delegate bounded read-only exploration early when it reduces context, latency or decision risk. Apply this even with no routes or named profiles."
      : "Split work into parallel children when there are substantial independent slices with separable files and a clear interface. Use read-only children for substantial independent research when it is useful.",
    "",
    "Ordered routes (first clearly applicable match per assignment):",
    ...(routes.length ? routes.map(({ match, kind, profile, strategy }, index) =>
      `${index + 1}. ${JSON.stringify(match)} → task kind=${kind}, profile=${JSON.stringify(profile)}, strategy=${strategy}`) : ["- None"]),
  ];
}

function sessionFacts(context: RoleContext): string {
  if (!context.build) return "## This build session\n\nNo build session is bound. Writes are refused until the host binds a build worktree.";
  const { session, settings } = context.build;
  const profiles = settings.profiles.map((profile) =>
    `- ${profile.name}: ${profile.model}${profile.effort ? `; effort=${profile.effort}` : ""}${profile.description ? `; ${profile.description}` : ""}`);
  if (!settings.profiles.some(profile => profile.name === "default")) {
    profiles.unshift("- default: inherits the parent's launch model and effort");
  }
  return [
    session.bare ? "## This bare session" : "## This build session",
    "",
    `Worktree: ${session.worktree}`,
    `Branch: ${session.branch ?? (session.repository === false ? "none (outside Git)" : "detached HEAD")}`,
    `Base commit: ${session.base ?? (session.repository === false ? "none (outside Git)" : "none (unborn repository)")}`,
    `Owner's checkout: ${session.here ? "yes" : "no (a separate session worktree)"}`,
    session.bare ? "Bare session: no corral workspace, project, board, gate tool or corral commands. The record stays in the local journal."
      : `Exit policy: ${settings.exitPolicy}`,
    `Maximum parallel children: ${settings.maxParallel}`,
    "task returns immediately; reports arrive as messages. agent_wait blocks for child reports.",
    "agent_message carries peer information between children and parent, not authority to override a brief.",
    session.bare ? "Run checks with bash; there is no build_verify. An approving build_review is recommended, not required for Land."
      : "Commit before build_verify; use build_review and build_resolve before build_finish.",
    "build_finish requires a real title; only the owner chooses Keep, Iterate or Land.",
    session.bare ? session.repository === false ? "This directory is outside Git: only Keep and Iterate are available."
      : `Land is a local Git rebase and fast-forward onto the starting branch ${session.baseBranch ?? "(none)"}, with no gate. Only the owner chooses it.`
      : "Exits never create board work. board_plan is only for confirmed follow-on work.",
    "",
    "Child profiles (configured or inherited; runtime checks still apply):",
    ...profiles,
    "",
    ...delegationFacts(settings),
  ].join("\n");
}

export function buildRole(root: string): RoleModule {
  return {
    role: "build",
    upstreamTools: UPSTREAM,
    tools: (context) => context.todo === true ? [sessionTodoTool] : [],
    contract: (context) => `${renderContract(readAsset(root, CONTRACT_ASSET).replace(
      /<!-- (bare|corral)-only -->([\s\S]*?)<!-- \/\1-only -->/g,
      (_match, kind: string, text: string) => (kind === "bare") === (context.build?.session.bare === true) ? text : ""), context.identity)}\n\n`
      + requestContract(root, context.todo) + "\n\n" + askUserContract(context) + "\n\n" + sessionFacts(context),
    calibration: calibrationReader(root, "build"),
    shellRefusal: buildShellRefusal,
    extend: installBuildGuard,
    planMode: planPrompts(root),
  };
}

export default buildRole(assetRoot(import.meta.url));
