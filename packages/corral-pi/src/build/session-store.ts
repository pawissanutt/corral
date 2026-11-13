import { randomBytes, randomInt } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { BuildLaunch, BuildSessionRecord } from "./types.ts";

export function sessionDir(dataRoot: string, id: string): string {
  if (!/^B-[a-z0-9]+$/.test(id)) throw new Error("Invalid build session id");
  return join(dataRoot, "build", id);
}

export function createSession(launch: BuildLaunch, worktree: Pick<BuildSessionRecord, "worktree" | "branch" | "base">): BuildSessionRecord {
  const now = new Date();
  const suffix = now.getTime().toString(36) + randomInt(36 * 36).toString(36).padStart(2, "0");
  const name = launch.name?.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "") || suffix;
  return { version: 1, id: `B-${suffix}`, name, created: now.toISOString(), updated: now.toISOString(),
    repoRoot: launch.repoRoot, ...worktree, here: launch.here,
    ...(launch.project ? { project: launch.project } : {}), ...(launch.workspaceRoot ? { workspaceRoot: launch.workspaceRoot } : {}) };
}

export function loadSession(dataRoot: string, id: string): BuildSessionRecord {
  const record = JSON.parse(readFileSync(join(sessionDir(dataRoot, id), "session.json"), "utf8")) as BuildSessionRecord;
  if (!record || record.version !== 1 || record.id !== id || typeof record.name !== "string" || !/^[a-z0-9-]{1,40}$/.test(record.name)
    || typeof record.created !== "string" || !Number.isFinite(Date.parse(record.created))
    || typeof record.updated !== "string" || !Number.isFinite(Date.parse(record.updated))
    || typeof record.here !== "boolean" || ![record.repoRoot, record.worktree].every(value => typeof value === "string" && isAbsolute(value))
    || !(record.branch === null || typeof record.branch === "string") || !(record.base === null || typeof record.base === "string")
    || [record.workspaceRoot, record.sessionFile].some(value => value !== undefined && (typeof value !== "string" || !isAbsolute(value)))
    || (record.project !== undefined && typeof record.project !== "string")) throw new Error(`Invalid build session record: ${id}`);
  return record;
}

export function listSessions(dataRoot: string): BuildSessionRecord[] {
  let entries: string[];
  try { entries = readdirSync(join(dataRoot, "build")); } catch { return []; }
  const records: BuildSessionRecord[] = [];
  for (const id of entries) {
    try { records.push(loadSession(dataRoot, id)); } catch { /* An unreadable session does not hide the remaining sessions. */ }
  }
  return records.sort((a, b) => Date.parse(b.updated) - Date.parse(a.updated) || Date.parse(b.created) - Date.parse(a.created) || b.id.localeCompare(a.id));
}

export function saveSession(dataRoot: string, record: BuildSessionRecord): void {
  const dir = sessionDir(dataRoot, record.id);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  chmodSync(dir, 0o700);
  const temporary = join(dir, `.session-${randomBytes(12).toString("hex")}.tmp`);
  try {
    writeFileSync(temporary, `${JSON.stringify(record)}\n`, { flag: "wx", mode: 0o600 });
    renameSync(temporary, join(dir, "session.json"));
  } finally { rmSync(temporary, { force: true }); }
}
