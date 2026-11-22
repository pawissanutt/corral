import { realpathSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { corralPiDirectory } from "../auth/paths.ts";
import { findWorkspaceRoot, modelFlags, modelSettings, qualifiedModel, rejectRemovedModelOptions, startupModelSetting } from "../host/config.ts";
import type { BuildLaunchOptions, BuildSessionRecord } from "./types.ts";
import { buildProfileArgs, readWorkspaceToml } from "./profiles.ts";
import { validateBuildId } from "./session-store.ts";
import { readBuildSettings } from "./settings.ts";

export interface LocalBuildOptions {
  readonly id?: string;
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
  { dataRoot: string; dryRun: boolean; session?: string; json?: true } | undefined {
  if (!argv.some(arg => /^--(?:prune|dry-run|session|json)(?:=|$)/.test(arg))) return undefined;
  const { values } = parseArgs({ args: [...argv], options: {
    prune: { type: "boolean" }, "dry-run": { type: "boolean" }, "data-dir": { type: "string" },
    session: { type: "string" }, json: { type: "boolean" },
  } });
  if (!values.prune) throw new Error("--dry-run, --session and --json require --prune");
  if (values.session !== undefined) validateBuildId(values.session);
  if (values.json && !values.session) throw new Error("--json requires --session");
  return { dataRoot: buildDataRoot(values["data-dir"], env, cwd), dryRun: values["dry-run"] === true,
    ...(values.session ? { session: values.session } : {}), ...(values.json ? { json: true as const } : {}) };
}

function resolvedRepo(path: string): string | undefined {
  try { return realpathSync(path); } catch { return undefined; }
}

export function parseBuildFlags(argv: readonly string[], assets: string, env = process.env,
  cwd = process.cwd()): { launch: BuildLaunchOptions; local: LocalBuildOptions; startupModel: string | null } {
  rejectRemovedModelOptions(argv);
  const options = {
    ...modelFlags, "data-dir": { type: "string" }, socket: { type: "string" }, corral: { type: "string" },
    project: { type: "string" }, profile: { type: "string" }, name: { type: "string" }, resume: { type: "string" },
    id: { type: "string" },
    here: { type: "boolean" }, headless: { type: "boolean" },
  } as const;
  const { values } = parseArgs({ args: [...argv], options });
  if (values.id !== undefined) {
    validateBuildId(values.id);
    if (values.resume !== undefined) throw new Error("--id and --resume are mutually exclusive");
  }
  // Resume binds to session.json, not to workspace discovery in the caller's directory.
  const resume = values.resume;
  const resuming = resume !== undefined;
  if (resuming) validateBuildId(resume);
  const workspaceRoot = resuming ? undefined : findWorkspaceRoot(env, cwd);
  const git = resuming ? undefined : spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8" });
  const repoRoot = git?.status === 0 ? git.stdout.trim() : cwd;
  const projects = workspaceRoot && readWorkspaceToml(workspaceRoot, "corral.toml").projects as Record<string, { path?: string }> | undefined;
  const project = values.project ?? (workspaceRoot && projects ? Object.entries(projects).find(([, entry]) =>
    typeof entry.path === "string" && resolvedRepo(resolve(workspaceRoot, entry.path)) === repoRoot)?.[0] : undefined);
  const corral = values.corral ? resolve(cwd, values.corral) : workspaceRoot ? "corral-next" : undefined;
  const dataRoot = buildDataRoot(values["data-dir"], env, cwd);
  const profileArgs = values.profile && !resuming ? buildProfileArgs(workspaceRoot ?? cwd, values.profile) : [];
  rejectRemovedModelOptions(profileArgs);
  const profile = parseArgs({ args: profileArgs, options: modelFlags }).values;
  const raw = resuming ? {} : readWorkspaceToml(workspaceRoot ?? repoRoot, "pi.toml");
  const settings = readBuildSettings({ workspaceRoot, repoRoot }, raw);
  const flags = { ...profile, ...values };
  const managed = !resuming && values.headless === true && values.id !== undefined && env.CORRAL_PI_MANAGED_BUILD === "1";
  if (managed && flags.model === undefined && flags.provider === undefined) {
    const saved = (raw.startup as { model?: unknown } | undefined)?.model;
    if (saved !== undefined) {
      if (typeof saved !== "string" || !qualifiedModel(saved)) throw new Error(".corral-next/pi.toml: startup.model must be a known <provider>/<model>");
      flags.model = saved;
    }
  }
  // Native daemon Builds keep the behavior of the existing Project default route.
  const route = managed ? modelSettings(flags) : modelSettings(flags, "startup");
  return {
    launch: { version: 1, assets, dataRoot, repoRoot, here: values.here === true,
      ...route, ...(corral ? { corral } : { bare: true }),
      ...(workspaceRoot ? { workspaceRoot } : {}), ...(corral && project ? { project } : {}),
      ...(values.name ? { name: values.name } : {}), ...(resuming ? { resume } : {}), settings },
    local: { headless: values.headless === true || !process.stdin.isTTY,
      ...(values.id !== undefined ? { id: values.id } : {}),
      ...(values.socket ? { socket: resolve(cwd, values.socket) } : {}) },
    startupModel: workspaceRoot ? startupModelSetting(raw) : null,
  };
}

export function bindBuildResume(launch: BuildLaunchOptions, record: BuildSessionRecord): BuildLaunchOptions {
  // A pre-route record must use the transcript selection, never a model from today's flags or workspace.
  return { ...launch, ...(record.route ?? { provider: undefined, model: undefined }),
    repoRoot: record.repoRoot, workspaceRoot: record.workspaceRoot, project: record.project,
    here: record.here, bare: record.bare, corral: record.bare ? undefined : launch.corral ?? "corral-next",
    settings: readBuildSettings(record) };
}
