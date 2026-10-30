// Role assets are read from the installation a role module was loaded from, never from the
// worktree, the owner's home or another harness's store.
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import type { HostIdentity } from "../types.ts";

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

/** Fills `{{task}}`, `{{run}}`, `{{review}}`, `{{project}}` and `{{assistant}}` from the launcher-bound identity. */
export function renderContract(template: string, identity: HostIdentity): string {
  const values: Record<string, string> = {
    task: identity.task ?? "", run: identity.run ?? "", review: identity.review ?? "", project: identity.project,
    assistant: identity.assistant ?? "",
  };
  return template.replace(/\{\{(task|run|review|project|assistant)\}\}/g, (_match, key: string) => values[key] ?? "");
}
