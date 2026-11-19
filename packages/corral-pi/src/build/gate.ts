import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { closeSync, mkdirSync, openSync, writeSync } from "node:fs";
import { join } from "node:path";
import { checked, type Run } from "./corral-cli.ts";
import { readWorkspaceToml } from "./profiles.ts";
import type { BuildJournal } from "./record.ts";
import type { BuildGateResult } from "./record-types.ts";
import type { BuildContext, BuildSessionRecord } from "./types.ts";

interface BuildProject {
  readonly base?: string;
  readonly gate?: string[];
  readonly env?: Record<string, string>;
}

export function buildProject({ workspaceRoot, project }: Pick<BuildSessionRecord, "workspaceRoot" | "project">): BuildProject {
  if (!workspaceRoot || !project) return {};
  const config = readWorkspaceToml(workspaceRoot, "corral.toml") as {
    defaults?: { base?: string }; projects?: Record<string, BuildProject>;
  };
  const selected = config.projects?.[project];
  if (!selected) throw new Error(`Project ${project} is missing from .corral-next/corral.toml`);
  return { ...selected, base: selected.base ?? config.defaults?.base };
}

/** What a caller follows while the gate runs. */
export interface GateWatch {
  /** The log exists and `commands` are about to run in order. */
  started?(log: string, commands: readonly string[]): void;
  /** `commands[index]` starts. */
  command?(index: number): void;
}

/** How long a stopped gate line gets to end after SIGTERM before its group is killed. */
const STOP_GRACE_MS = 2_000;

/**
 * Runs one gate line in its own process group, so stopping it also stops what it started. The group is
 * signalled only while its leader runs: a group ID whose leader is gone may belong to someone else.
 */
async function runLine(command: string, options: { cwd: string; env: NodeJS.ProcessEnv; fd: number; signal?: AbortSignal }): Promise<number | null> {
  const { promise, resolve, reject } = Promise.withResolvers<number | null>();
  const child = spawn("sh", ["-c", command], { cwd: options.cwd, env: options.env, stdio: ["ignore", options.fd, options.fd], detached: true });
  let running = true, grace: ReturnType<typeof setTimeout> | undefined;
  const kill = (signal: NodeJS.Signals) => {
    if (!running || child.pid === undefined) return;
    try { process.kill(-child.pid, signal); } catch { /* The group already ended. */ }
  };
  const stop = () => { kill("SIGTERM"); grace = setTimeout(() => kill("SIGKILL"), STOP_GRACE_MS); };
  const hostExit = () => kill("SIGKILL");
  child.once("error", reject);
  child.once("exit", () => { running = false; });
  child.once("close", resolve);
  if (options.signal?.aborted) stop(); else options.signal?.addEventListener("abort", stop, { once: true });
  process.once("exit", hostExit);
  try { return await promise; }
  finally {
    running = false;
    clearTimeout(grace);
    options.signal?.removeEventListener("abort", stop);
    process.off("exit", hostExit);
  }
}

export async function verifyBuild(context: BuildContext, git: Run, journal: BuildJournal, signal?: AbortSignal, watch?: GateWatch): Promise<BuildGateResult> {
  const { worktree } = context.session;
  if ((await checked(git, ["status", "--porcelain"], worktree)).trim()) throw new Error("Commit your changes first, then run build_verify again.");
  const head = (await checked(git, ["rev-parse", "HEAD"], worktree)).trim();
  const { gate: commands = [], env } = buildProject(context.session);
  const started = new Date().toISOString();
  let failed: string | undefined;
  let log: string | undefined;
  if (commands.length) {
    const dir = join(context.dir, "gates");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    log = join(dir, `${randomUUID()}.log`);
    const fd = openSync(log, "wx", 0o600);
    try {
      watch?.started?.(log, commands);
      for (const [index, command] of commands.entries()) {
        signal?.throwIfAborted();
        writeSync(fd, `$ ${command}\n`);
        watch?.command?.(index);
        const code = await runLine(command, { cwd: worktree, env: { ...process.env, ...env }, fd, signal });
        // A stopped gate has no result: it is neither a pass nor this command's failure.
        signal?.throwIfAborted();
        if (code !== 0) { failed = command; break; }
      }
    } finally { closeSync(fd); }
  }
  const previous = Number(journal.latestGate()?.attempt.slice(1) ?? 0);
  const result: BuildGateResult = { attempt: `G${previous + 1}`, head, commands, started, ended: new Date().toISOString(),
    result: failed ? "fail" : commands.length ? "pass" : "skipped", ...(failed ? { failed_command: failed } : {}), ...(log ? { log } : {}) };
  journal.gate(result);
  return result;
}
