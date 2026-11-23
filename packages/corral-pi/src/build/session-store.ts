import { randomBytes, randomInt } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { BuildLaunch, BuildSessionRecord } from "./types.ts";
import { isProviderId } from "../auth/providers.ts";

export function validateBuildId(id: string): void {
  if (!/^B-[a-z0-9]{1,32}$/.test(id)) throw new Error("Invalid build session id");
}

export function sessionDir(dataRoot: string, id: string): string {
  validateBuildId(id);
  return join(dataRoot, "build", id);
}

export function createSession(launch: BuildLaunch, worktree: Pick<BuildSessionRecord, "worktree" | "branch" | "base">,
  id?: string): BuildSessionRecord {
  const now = new Date();
  id ??= `B-${now.getTime().toString(36)}${randomInt(36 * 36).toString(36).padStart(2, "0")}`;
  validateBuildId(id);
  const name = launch.name?.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "") || id.slice(2);
  const title = launch.name?.trim();
  return { version: 1, id, name, ...(title ? { title } : {}), created: now.toISOString(), updated: now.toISOString(),
    repoRoot: launch.repoRoot, ...worktree, here: launch.here, ...(launch.bare ? { bare: true } : {}),
    ...(launch.trustedCode ? { trustedCode: true } : {}),
    route: {
      provider: launch.provider, model: launch.model, effort: launch.effort, aws: launch.aws, fallback: launch.fallback,
      ...(launch.noFallback ? { noFallback: true } : {}),
    },
    ...(launch.project ? { project: launch.project } : {}), ...(launch.workspaceRoot ? { workspaceRoot: launch.workspaceRoot } : {}) };
}

export function loadSession(dataRoot: string, id: string): BuildSessionRecord {
  const record = JSON.parse(readFileSync(join(sessionDir(dataRoot, id), "session.json"), "utf8")) as BuildSessionRecord;
  if (!record || record.version !== 1 || record.id !== id || typeof record.name !== "string" || !/^[a-z0-9-]{1,40}$/.test(record.name)
    || typeof record.created !== "string" || !Number.isFinite(Date.parse(record.created))
    || typeof record.updated !== "string" || !Number.isFinite(Date.parse(record.updated))
    || typeof record.here !== "boolean" || ![record.repoRoot, record.worktree].every(value => typeof value === "string" && isAbsolute(value))
    || !(record.branch === null || typeof record.branch === "string") || !(record.base === null || typeof record.base === "string")
    || [record.workspaceRoot, record.sessionFile, record.socket].some(value => value !== undefined && (typeof value !== "string" || !isAbsolute(value)))
    || (record.trustedCode !== undefined && (record.trustedCode !== true || !record.here))
    || (record.bare !== undefined && typeof record.bare !== "boolean")
    || (record.repository !== undefined && typeof record.repository !== "boolean")
    || (record.baseBranch !== undefined && record.baseBranch !== null && typeof record.baseBranch !== "string")
    || (record.pid !== undefined && (!Number.isSafeInteger(record.pid) || record.pid <= 0))
    || (record.cleaned !== undefined && record.cleaned !== true)
    || (record.bare === true && (record.workspaceRoot !== undefined || record.project !== undefined))
    || (record.project !== undefined && typeof record.project !== "string")
    || (record.title !== undefined && typeof record.title !== "string")) throw new Error(`Invalid build session record: ${id}`);
  if (record.route) {
    if (!isProviderId(record.route.provider) || typeof record.route.model !== "string" ||
        (record.route.noFallback !== undefined && typeof record.route.noFallback !== "boolean") ||
        (record.route.fallback !== undefined && (!Array.isArray(record.route.fallback) ||
          record.route.fallback.some(choice => !choice || !isProviderId(choice.provider) || typeof choice.model !== "string" ||
            !choice.model || /[\s\0]/.test(choice.model))))) throw new Error(`Invalid Build model route: ${id}`);
    const { connection, ...route } = record.route as typeof record.route & { connection?: unknown };
    // Historical Anthropic API builds stored the upstream name, never the subscription.
    if (connection === "anthropic-api" && route.provider === "anthropic") {
      return { ...record, route: { ...route, provider: "anthropic-api", model: route.model.replace(/^anthropic\//, "") } };
    }
    if (connection !== undefined) {
      const historical = { "openai-codex": "openai-subscription", anthropic: "anthropic-subscription", openai: "openai-api",
        "anthropic-api": undefined, "amazon-bedrock": "amazon-bedrock", "llama.cpp": "llama.cpp" };
      if (connection !== historical[route.provider]) throw new Error(`Invalid Build model route: ${id}`);
      return { ...record, route };
    }
  }
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
  const path = join(dir, "session.json");
  let socketState = {};
  try {
    const { role, pid, paused, held_input, socket } = JSON.parse(readFileSync(path, "utf8"));
    if (role === "build") socketState = { role, pid: record.pid ?? pid, paused, held_input, socket: record.socket ?? socket };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const temporary = join(dir, `.session-${randomBytes(12).toString("hex")}.tmp`);
  try {
    writeFileSync(temporary, `${JSON.stringify({ ...record, ...socketState })}\n`, { flag: "wx", mode: 0o600 });
    renameSync(temporary, path);
  } finally { rmSync(temporary, { force: true }); }
}
