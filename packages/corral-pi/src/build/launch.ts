import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { corralPiDirectory } from "../auth/paths.ts";
import { findWorkspaceRoot, modelFlags, modelSettings, rejectRemovedModelOptions } from "../host/config.ts";
import { DEFAULT_MAX_PARALLEL, type BuildLaunch } from "./types.ts";
import { buildProfileArgs, readBuildProfiles, readWorkspaceToml } from "./profiles.ts";

export interface LocalBuildOptions {
  readonly socket?: string;
  readonly headless: boolean;
}

function buildDataRoot(value: string | undefined, env: NodeJS.ProcessEnv, cwd: string): string {
  // Do not create the directory whose absence keeps a legacy installation's logins in use.
  const data = env.XDG_DATA_HOME;
  return value ? resolve(cwd, value)
    : corralPiDirectory(join(data && isAbsolute(data) ? data : join(homedir(), ".local", "share"), "corral", "corral-pi"));
}

export function parsePruneFlags(argv: readonly string[], env = process.env, cwd = process.cwd()):
  { dataRoot: string; dryRun: boolean } | undefined {
  if (!argv.some(arg => /^--(?:prune|dry-run)(?:=|$)/.test(arg))) return undefined;
  const { values } = parseArgs({ args: [...argv], options: {
    prune: { type: "boolean" }, "dry-run": { type: "boolean" }, "data-dir": { type: "string" },
  } });
  if (!values.prune) throw new Error("--dry-run requires --prune");
  return { dataRoot: buildDataRoot(values["data-dir"], env, cwd), dryRun: values["dry-run"] === true };
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
  const workspaceRoot = findWorkspaceRoot(env, cwd);
  const git = spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8" });
  const repoRoot = git.status === 0 ? git.stdout.trim() : cwd;
  const projects = workspaceRoot && readWorkspaceToml(workspaceRoot, "corral.toml").projects as Record<string, { path?: string }> | undefined;
  const project = values.project ?? (workspaceRoot && projects ? Object.entries(projects).find(([, entry]) =>
    typeof entry.path === "string" && resolve(workspaceRoot, entry.path) === repoRoot)?.[0] : undefined);
  const corral = values.corral ? resolve(cwd, values.corral) : workspaceRoot ? "corral-next" : undefined;
  const dataRoot = buildDataRoot(values["data-dir"], env, cwd);
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
