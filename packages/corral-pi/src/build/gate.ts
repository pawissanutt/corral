import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
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

type GateConfig = Pick<BuildProject, "gate" | "env">;

/** Identity of explicit project inputs only; ambient credentials are not hashed. */
export function gateInputFingerprint({ gate = [], env = {} }: GateConfig): string {
  if (!Array.isArray(gate) || gate.some(command => typeof command !== "string")) throw new Error("The project gate must be an array of commands.");
  if (!env || typeof env !== "object" || Array.isArray(env)) throw new Error("The project env must be a table of strings.");
  const entries = Object.entries(env).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  if (entries.some(([, value]) => typeof value !== "string")) throw new Error("The project env must be a table of strings.");
  return createHash("sha256").update(JSON.stringify(["corral-project-gate-v1", gate, entries])).digest("hex");
}

/** What a caller follows while the gate runs. */
export interface GateWatch {
  /** The log exists and `commands` are about to run in order. */
  started?(log: string, commands: readonly string[]): void;
  /** `commands[index]` starts. */
  command?(index: number): void;
  tick?(): void;
}

/** Additional prepared-candidate identity; safe to check alongside a live reviewer. */
interface GatePin {
  readonly head: string;
  readonly inputFingerprint: string;
  check(): Promise<void>;
}

const GATE_WATCH_MS = 500;

/** How long what a stopped gate line started gets to end after SIGTERM before its group is killed. */
const STOP_GRACE_S = 1;

/**
 * The group's leader. It runs the gate line (`$1`) in the foreground, with the signal dispositions the line always
 * had, and ends with its status. SIGTERM stops the line, not the leader: a shell runs its trap once the foreground
 * command has ended, so the leader then gives what the line started the grace and kills its own group. A process
 * that ignores SIGTERM does not outlive a stopped gate; a line that ends by itself leaves its group alone.
 */
const LEADER = [
  "stop=", "trap 'stop=1' TERM", 'sh -c "$1"', "s=$?",
  `[ -n "$stop" ] && { sleep ${STOP_GRACE_S}; kill -KILL 0; }`, 'exit "$s"',
].join("\n");

/**
 * Runs one gate line in its own process group, so stopping it also stops what it started. The group is
 * signalled only while its leader runs: a group ID whose leader is gone may belong to someone else. The leader
 * outlives a stop, so the kill after the grace reaches a line that ignores SIGTERM itself.
 */
async function runLine(command: string, options: { cwd: string; env: NodeJS.ProcessEnv; fd: number; signal?: AbortSignal }): Promise<number | null> {
  const { promise, resolve, reject } = Promise.withResolvers<number | null>();
  const child = spawn("sh", ["-c", LEADER, "sh", command], { cwd: options.cwd, env: options.env, stdio: ["ignore", options.fd, options.fd], detached: true });
  let running = true, grace: ReturnType<typeof setTimeout> | undefined;
  const kill = (signal: NodeJS.Signals) => {
    if (!running || child.pid === undefined) return;
    try { process.kill(-child.pid, signal); } catch { /* The group already ended. */ }
  };
  const stop = () => { kill("SIGTERM"); grace = setTimeout(() => kill("SIGKILL"), STOP_GRACE_S * 1000 + 500); };
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

const configChanged = () => new Error("The configured gate inputs changed. Run build_verify again.");
const readHead = async (git: Run, worktree: string) => (await checked(git, ["rev-parse", "HEAD"], worktree)).trim();

export async function checkGateSource(git: Run, worktree: string, head: string): Promise<void> {
  // Porcelain includes both the staged and unstaged columns. Don't refresh/write the index just to observe it.
  if ((await checked(git, ["--no-optional-locks", "status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"], worktree)).trim()) {
    throw new Error("The build worktree or index is dirty. Commit or stash your changes, then run build_verify again.");
  }
  if (await readHead(git, worktree) !== head) throw new Error("Build HEAD changed. Run build_verify on the current candidate.");
}

export async function verifyBuild(context: BuildContext, git: Run, journal: BuildJournal, signal?: AbortSignal, watch?: GateWatch,
  pin?: GatePin): Promise<BuildGateResult> {
  const { worktree } = context.session;
  signal?.throwIfAborted();
  const project = buildProject(context.session);
  const inputFingerprint = gateInputFingerprint(project);
  if (pin && pin.inputFingerprint !== inputFingerprint) throw configChanged();
  const { gate: commands = [], env } = project;
  const executionEnv = { ...process.env, ...env };
  const head = pin?.head ?? await readHead(git, worktree);
  const controller = new AbortController();
  const stop = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal;
  async function check() {
    stop.throwIfAborted();
    await pin?.check();
    await checkGateSource(git, worktree, head);
    if (gateInputFingerprint(buildProject(context.session)) !== inputFingerprint) throw configChanged();
    stop.throwIfAborted();
  }
  await check();
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
      let checking: Promise<void> | undefined;
      const timer = setInterval(() => {
        watch?.tick?.();
        if (checking || stop.aborted) return;
        // This samples the live checkout; it doesn't isolate the gate from other writers. Latch the first
        // observed change so restoring or stashing it never restores the run's validity.
        checking = check().catch(error => controller.abort(error)).finally(() => { checking = undefined; });
      }, GATE_WATCH_MS);
      try {
        for (const [index, command] of commands.entries()) {
          writeSync(fd, `$ ${command}\n`);
          watch?.command?.(index);
          await check();
          const code = await runLine(command, { cwd: worktree, env: executionEnv, fd, signal: stop });
          // A stopped gate has no result: it is neither a pass nor this command's failure.
          stop.throwIfAborted();
          if (code !== 0) { failed = command; break; }
        }
      } finally {
        clearInterval(timer);
        await checking;
      }
    } finally { closeSync(fd); }
  }
  // Always recheck after commands stop and after any in-flight monitor check settles, immediately before recording.
  await check();
  const previous = Number(journal.latestGate()?.attempt.slice(1) ?? 0);
  const result: BuildGateResult = { attempt: `G${previous + 1}`, head, input_fingerprint: inputFingerprint, commands, started, ended: new Date().toISOString(),
    result: failed !== undefined ? "fail" : commands.length ? "pass" : "skipped", ...(failed !== undefined ? { failed_command: failed } : {}), ...(log ? { log } : {}) };
  journal.gate(result);
  return result;
}
