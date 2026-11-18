import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { modelFlags, modelSettings } from "../host/config.ts";
import { DEFAULT_MAX_PARALLEL, type BuildLaunch } from "./types.ts";

export interface LocalBuildOptions {
  readonly socket?: string;
  readonly headless: boolean;
}

export function parseBuildFlags(argv: readonly string[], assets: string, env = process.env,
  cwd = process.cwd()): { launch: BuildLaunch; local: LocalBuildOptions } {
  const { values } = parseArgs({ args: [...argv], options: {
    ...modelFlags, "data-dir": { type: "string" }, socket: { type: "string" }, corral: { type: "string" },
    project: { type: "string" }, name: { type: "string" }, resume: { type: "string" }, here: { type: "boolean" },
    headless: { type: "boolean" },
  } });
  const workspaceRoot = env.CORRAL_NEXT_ROOT ? resolve(cwd, env.CORRAL_NEXT_ROOT) : undefined;
  const corral = values.corral ? resolve(cwd, values.corral) : workspaceRoot ? "corral-next" : undefined;
  const dataRoot = values["data-dir"] ? resolve(cwd, values["data-dir"])
    : join(env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "corral", "corral-pi");
  return {
    launch: { version: 1, assets, dataRoot, repoRoot: cwd, here: values.here === true,
      ...modelSettings(values), ...(corral ? { corral } : { bare: true }),
      ...(workspaceRoot ? { workspaceRoot } : {}), ...(corral && values.project ? { project: values.project } : {}),
      ...(values.name ? { name: values.name } : {}), ...(values.resume ? { resume: values.resume } : {}),
      settings: { exitPolicy: workspaceRoot ? "review" : "none", maxParallel: DEFAULT_MAX_PARALLEL, profiles: [] } },
    local: { headless: values.headless === true || !process.stdin.isTTY,
      ...(values.socket ? { socket: resolve(cwd, values.socket) } : {}) },
  };
}
