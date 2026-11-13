import { randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { ProviderId } from "../auth/providers.ts";
import type { CapabilityGuidance } from "../capabilities/guidance.ts";
import { LocalControlPlane } from "../build/control-plane.ts";
import { readBuildLaunch } from "../build/launch.ts";
import { createSession, loadSession, saveSession, sessionDir } from "../build/session-store.ts";
import { createTodoLedger } from "../build/todo-ledger.ts";
import { prepareWorktree } from "../build/worktree.ts";
import type { BuildContext, BuildLaunch, BuildSessionRecord } from "../build/types.ts";
import type { HostCapabilities, HostConfig } from "./config.ts";
import { runHost } from "./main.ts";

// Everything here runs in the host, its own managed processes, or the local receipt plane.
// write_permissions needs assistant scratch ownership; checkpoints needs a worker run;
// hooks is a worker-only control;
// session_export needs corral artifacts; headless needs a bound launcher lifecycle;
// shadow and expert need PM state; role_enabled is planning-role admission;
// llama_cpp.enabled needs a generation-bound local inference route. budget is not an
// optional build capability: local accounting is unlimited and handled for every request.
// context_files and prompt_templates are excluded to avoid opting a standalone build
// into the bound project's context-file and prompt-template discovery policy.
export const BUILD_CAPABILITY_KEYS: readonly string[] = [
  "todo", "queue_control", "web.enabled", "mcp.enabled", "compaction", "browser", "subagents", "oracle",
  "ask_user", "codemode", "tool_search", "lsp", "skills", "bash_rules", "background_shell",
];

class BuildResumeRefused extends Error {
  readonly code = "build_resume_refused";
  constructor(path: string, reason: string) { super(`build_resume_refused: ${path}: ${reason}`); }
}

/** Verify the recorded checkout before opening its transcript or binding any tools. */
export function validateBuildResume(record: BuildSessionRecord): void {
  const commonDirectory = (path: string): string => {
    const result = spawnSync("git", ["-C", path, "rev-parse", "--git-common-dir"], { encoding: "utf8" });
    if (result.error || result.status !== 0) {
      throw new BuildResumeRefused(record.worktree, `cannot read Git common directory for ${path}: ${result.stderr?.trim() || result.error?.message || `exit ${result.status}`}`);
    }
    try { return realpathSync(resolve(path, result.stdout.trim())); }
    catch (error) {
      throw new BuildResumeRefused(record.worktree, `cannot resolve Git common directory for ${path}: ${error instanceof Error ? error.message : String(error)}`);
    }
  };
  const expected = commonDirectory(record.repoRoot);
  const observed = commonDirectory(record.worktree);
  if (observed !== expected) {
    throw new BuildResumeRefused(record.worktree, `Git common directory differs: expected ${expected}, found ${observed}`);
  }
  if (record.here) return;
  const result = spawnSync("git", ["-C", record.worktree, "symbolic-ref", "--quiet", "--short", "HEAD"], { encoding: "utf8" });
  if (result.error || result.status !== 0 && result.status !== 1) {
    throw new BuildResumeRefused(record.worktree, `cannot read checked-out branch: ${result.stderr?.trim() || result.error?.message || `exit ${result.status}`}`);
  }
  const branch = result.status === 0 ? result.stdout.trim() : null;
  if (!record.branch || branch !== record.branch) {
    throw new BuildResumeRefused(record.worktree, `branch differs: expected ${record.branch ?? "a recorded build branch"}, found ${branch ?? "detached HEAD"}`);
  }
}

/** Build the same rich-host configuration for a new build and an explicitly resumed one. */
export function buildHostConfig(launch: BuildLaunch, record: BuildSessionRecord, assets: string): HostConfig {
  const dir = sessionDir(launch.dataRoot, record.id);
  // readBuildLaunch has validated these snapshots with the rich-launch parsers.
  const snapshot = launch.capabilities as HostCapabilities | undefined;
  const guidanceSnapshot = launch.guidance as CapabilityGuidance | undefined;
  const capabilities: HostCapabilities = {
    catalog: snapshot?.catalog ?? 10, digest: snapshot?.digest ?? "0".repeat(64), preset: snapshot?.preset ?? "default",
    enabled: snapshot?.enabled.filter(key => BUILD_CAPABILITY_KEYS.includes(key)) ?? [],
    ...(snapshot?.web ? { web: snapshot.web } : {}), ...(snapshot?.mcp ? { mcp: snapshot.mcp } : {}),
  };
  const guidance = guidanceSnapshot ? { ...guidanceSnapshot, settings: Object.fromEntries(
    Object.entries(guidanceSnapshot.settings).filter(([key]) => BUILD_CAPABILITY_KEYS.includes(key)
      || key === "web" && capabilities.enabled.includes("web.enabled") || key === "mcp" && capabilities.enabled.includes("mcp.enabled"))) } : undefined;
  return {
    identity: { store: "build", project: record.project ?? "", role: "build", generation: 1 },
    secrets: { hostCap: randomBytes(32).toString("base64url"), ctlCap: randomBytes(32).toString("base64url"), bindNonce: randomBytes(32).toString("base64url") },
    pane: "", cwd: record.worktree, root: record.workspaceRoot ?? record.repoRoot, corral: launch.corral, assets,
    sessionDir: join(dir, "sessions"), runtimeDir: join(dir, "runtime"), ui: "rich",
    ...(record.sessionFile ? { resumeSession: record.sessionFile } : {}), capabilities,
    ...(guidance ? { guidance } : {}), todo: capabilities.enabled.includes("todo"),
    provider: launch.provider as ProviderId, model: launch.model, ...(launch.effort ? { effort: launch.effort } : {}),
    ...(launch.aws ? { aws: launch.aws } : {}),
  };
}

export async function buildCommand(argv: readonly string[], assets: string): Promise<void> {
  try {
    if (argv.length !== 2 || argv[0] !== "--launch-fd" || !/^[0-9]+$/.test(argv[1] ?? "")) {
      throw new Error("Run with --launch-fd <n>");
    }
    const fd = Number(argv[1]);
    if (!Number.isSafeInteger(fd) || fd < 3) throw new Error("Invalid launch descriptor");
    const launch = readBuildLaunch(fd);
    if (launch.provider === "llama.cpp") throw new Error("Local inference is unavailable in a build session");
    let record: BuildSessionRecord;
    if (launch.resume) {
      record = loadSession(launch.dataRoot, launch.resume);
      validateBuildResume(record);
      if (!record.sessionFile) throw new Error("The build session has no recorded transcript to resume");
    } else {
      const draft = createSession(launch, { worktree: launch.repoRoot, branch: null, base: null });
      record = { ...draft, ...prepareWorktree(launch.repoRoot, draft.name, launch.here) };
      saveSession(launch.dataRoot, record);
    }
    const dir = sessionDir(launch.dataRoot, record.id);
    const build: BuildContext = { session: record, settings: launch.settings, dir, corral: launch.corral };
    const config = buildHostConfig(launch, record, assets);
    const ledger = createTodoLedger(dir);
    await runHost(config, { bridge: false, build, client: (identity, sessionId) => new LocalControlPlane({
      dir, root: config.root, identity, sessionId, handlers: { todo: ledger.handle }, todoSnapshot: () => ledger.snapshot(),
    }) });
  } catch (error) {
    process.stderr.write(`corral pi --build stopped: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(2);
  }
}

if (import.meta.main) await buildCommand(process.argv.slice(2), fileURLToPath(new URL("../..", import.meta.url)).replace(/\/$/, ""));
