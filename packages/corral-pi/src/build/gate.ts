import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { closeSync, mkdirSync, openSync, writeSync } from "node:fs";
import { join } from "node:path";
import { checked, type Run } from "./corral-cli.ts";
import { readWorkspaceToml } from "./profiles.ts";
import type { BuildJournal } from "./record.ts";
import type { BuildGateResult } from "./record-types.ts";
import type { BuildContext } from "./types.ts";

interface BuildProject {
  readonly base?: string;
  readonly gate?: string[];
  readonly env?: Record<string, string>;
}

export function buildProject(context: BuildContext): BuildProject {
  const { workspaceRoot, project } = context.session;
  if (!workspaceRoot || !project) return {};
  const config = readWorkspaceToml(workspaceRoot, "corral.toml") as {
    defaults?: { base?: string }; projects?: Record<string, BuildProject>;
  };
  const selected = config.projects?.[project];
  if (!selected) throw new Error(`Project ${project} is missing from .corral-next/corral.toml`);
  return { ...selected, base: selected.base ?? config.defaults?.base };
}

export async function verifyBuild(context: BuildContext, git: Run, journal: BuildJournal, signal?: AbortSignal): Promise<BuildGateResult> {
  const { worktree } = context.session;
  if ((await checked(git, ["status", "--porcelain"], worktree)).trim()) throw new Error("Commit your changes first, then run build_verify again.");
  const head = (await checked(git, ["rev-parse", "HEAD"], worktree)).trim();
  const { gate: commands = [], env } = buildProject(context);
  const started = new Date().toISOString();
  let failed: string | undefined;
  let log: string | undefined;
  if (commands.length) {
    const dir = join(context.dir, "gates");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    log = join(dir, `${randomUUID()}.log`);
    const fd = openSync(log, "wx", 0o600);
    try {
      for (const command of commands) {
        signal?.throwIfAborted();
        writeSync(fd, `$ ${command}\n`);
        const { promise, resolve, reject } = Promise.withResolvers<number | null>();
        const child = spawn("sh", ["-c", command], {
          cwd: worktree, env: { ...process.env, ...env }, stdio: ["ignore", fd, fd], signal,
        });
        child.once("error", reject);
        child.once("close", resolve);
        if (await promise !== 0) { failed = command; break; }
      }
    } finally { closeSync(fd); }
  }
  const previous = Number(journal.latestGate()?.attempt.slice(1) ?? 0);
  const result: BuildGateResult = { attempt: `G${previous + 1}`, head, commands, started, ended: new Date().toISOString(),
    result: failed ? "fail" : commands.length ? "pass" : "skipped", ...(failed ? { failed_command: failed } : {}), ...(log ? { log } : {}) };
  journal.gate(result);
  return result;
}
