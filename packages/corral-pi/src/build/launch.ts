import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { corralPiDirectory } from "../auth/paths.ts";
import { modelFlags, modelSettings, rejectRemovedModelOptions } from "../host/config.ts";
import { DEFAULT_MAX_PARALLEL, type BuildLaunch } from "./types.ts";
import { buildProfileArgs, readBuildProfiles, readWorkspaceToml } from "./profiles.ts";

export interface LocalBuildOptions {
  readonly socket?: string;
  readonly headless: boolean;
}

export function parseBuildFlags(argv: readonly string[], assets: string, env = process.env,
  cwd = process.cwd()): { launch: BuildLaunch; local: LocalBuildOptions } {
  rejectRemovedModelOptions(argv);
  const options = {
    ...modelFlags, "data-dir": { type: "string" }, socket: { type: "string" }, corral: { type: "string" },
    project: { type: "string" }, profile: { type: "string" }, name: { type: "string" }, resume: { type: "string" },
    here: { type: "boolean" }, headless: { type: "boolean" },
  } as const;
  const { values } = parseArgs({ args: [...argv], options });
  let workspaceRoot = env.CORRAL_NEXT_ROOT ? resolve(cwd, env.CORRAL_NEXT_ROOT) : undefined;
  if (!workspaceRoot) for (let cur = cwd; ; cur = dirname(cur)) {
    if (existsSync(join(cur, ".corral-next", "corral.toml"))) { workspaceRoot = cur; break; }
    if (cur === dirname(cur)) break;
  }
  const git = spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8" });
  const repoRoot = git.status === 0 ? git.stdout.trim() : cwd;
  const projects = workspaceRoot && readWorkspaceToml(workspaceRoot, "corral.toml").projects as Record<string, { path?: string }> | undefined;
  const project = values.project ?? (workspaceRoot && projects ? Object.entries(projects).find(([, entry]) =>
    typeof entry.path === "string" && resolve(workspaceRoot, entry.path) === repoRoot)?.[0] : undefined);
  const corral = values.corral ? resolve(cwd, values.corral) : workspaceRoot ? "corral-next" : undefined;
  // A build must not create the directory whose absence keeps a legacy installation's logins in use.
  const dataRoot = values["data-dir"] ? resolve(cwd, values["data-dir"])
    : corralPiDirectory(join(env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "corral", "corral-pi"));
  const profileArgs = values.profile ? buildProfileArgs(workspaceRoot ?? cwd, values.profile) : [];
  rejectRemovedModelOptions(profileArgs);
  const profile = parseArgs({ args: profileArgs, options: modelFlags }).values;
  return {
    launch: { version: 1, assets, dataRoot, repoRoot, here: values.here === true,
      ...modelSettings({ ...profile, ...values }), ...(corral ? { corral } : { bare: true }),
      ...(workspaceRoot ? { workspaceRoot } : {}), ...(corral && project ? { project } : {}),
      ...(values.name ? { name: values.name } : {}), ...(values.resume ? { resume: values.resume } : {}),
      settings: { exitPolicy: workspaceRoot ? "review" : "none", maxParallel: DEFAULT_MAX_PARALLEL,
        profiles: readBuildProfiles(workspaceRoot ?? cwd) } },
    local: { headless: values.headless === true || !process.stdin.isTTY,
      ...(values.socket ? { socket: resolve(cwd, values.socket) } : {}) },
  };
}
