// Role assets are read from the installation a role module was loaded from, never from the
// worktree, the owner's home or another harness's store.
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import type { HostIdentity } from "../types.ts";

export const WORKER_SKILL_ASSET = "src/roles/worker/skills/corral-worker/SKILL.md";
export const PLAN_SKILL_ASSET = "assets/skills/corral-plan/SKILL.md";
export const DIAGRAM_SKILL_ASSET = "assets/skills/corral-diagrams/SKILL.md";

export const CODING_SKILL_NAMES = ["debugging", "frontend", "git-master", "programming", "simplify-and-cleanup", "visual-qa"] as const;

export function codingSkillPaths(root: string): string[] {
  return CODING_SKILL_NAMES.map(name => assetPath(root, `assets/skills/${name}/SKILL.md`));
}

export class RoleAssetMissing extends Error {
  readonly code = "role_asset_missing";
  constructor(readonly path: string) {
    super(`role_asset_missing: ${path} is not in this installation`);
  }
}

/** The asset root of a role module at src/roles/<role>/index.ts. */
export function assetRoot(moduleUrl: string): string {
  return join(dirname(fileURLToPath(moduleUrl)), "..", "..", "..");
}

/** A regular file under the asset root; a missing or non-regular file is RoleAssetMissing. */
export function assetPath(root: string, relative: string): string {
  const path = join(root, relative);
  if (!existsSync(path) || !lstatSync(path).isFile()) throw new RoleAssetMissing(relative);
  return path;
}

export function readAsset(root: string, relative: string): string {
  return readFileSync(assetPath(root, relative), "utf8");
}

/** Fill the bound identity in the role's shipped contract. */
export function renderContract(template: string, identity: HostIdentity): string {
  const values: Record<string, string> = {
    task: identity.task ?? "", run: identity.run ?? "", project: identity.project,
    assistant: identity.assistant ?? "",
  };
  return template.replace(/\{\{(task|run|project|assistant)\}\}/g, (_match, key: string) => values[key] ?? "");
}
