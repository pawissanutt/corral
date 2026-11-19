import { closeSync, constants, copyFileSync, fstatSync, ftruncateSync, openSync, readSync, readdirSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { SessionManager } from "@earendil-works/pi-coding-agent";
import { readSessionRecord } from "./session-record.ts";
import { isProviderId } from "../auth/providers.ts";
import type { ModelChoice } from "./config.ts";

export type SessionSdk = Pick<typeof Sdk, "SessionManager" | "CURRENT_SESSION_VERSION">;

export class SessionResumeRefused extends Error {
  constructor(path: string, cause: unknown) {
    super(`session_resume_refused: ${path}: ${cause instanceof Error ? cause.message : String(cause)}`, { cause });
  }
}

/** A worker retry inherits the newest earlier run with a transcript, not a reviewer or a new run. */
export function siblingTranscript(dir: string): string | undefined {
  const current = /^R(\d+)$/.exec(basename(dir));
  if (!current?.[1]) return undefined;
  const parent = dirname(dir);
  const currentRun = BigInt(current[1]);
  const siblings = readdirSync(parent, { withFileTypes: true }).flatMap(entry => {
    const match = /^R(\d+)$/.exec(entry.name);
    if (!entry.isDirectory() || !match?.[1]) return [];
    const number = BigInt(match[1]);
    return number < currentRun ? [{ name: entry.name, number }] : [];
  }).sort((a, b) => a.number > b.number ? -1 : a.number < b.number ? 1 : 0);
  for (const sibling of siblings) {
    const saved = readSessionRecord(join(parent, sibling.name));
    if (saved.role === "worker" && typeof saved.sessionFile === "string") return saved.sessionFile;
  }
  return undefined;
}

/** Keep each run's archived transcript immutable while carrying forward its conversation. */
export function inheritTranscript(source: string, sessionDir: string): string {
  const path = join(sessionDir, `continued-${crypto.randomUUID()}.jsonl`);
  copyFileSync(source, path, constants.COPYFILE_EXCL);
  return path;
}

/** A stopped process may leave an unterminated append. The newline commits the JSONL record. */
function discardIncompleteTail(fd: number, size: number): void {
  const page = Buffer.allocUnsafe(4096);
  for (let end = size; end > 0;) {
    const start = Math.max(0, end - page.length);
    const length = readSync(fd, page, 0, end - start, start);
    const newline = page.subarray(0, length).lastIndexOf(0x0a);
    if (newline >= 0) {
      const complete = start + newline + 1;
      if (complete < size) ftruncateSync(fd, complete);
      return;
    }
    end = start;
  }
  throw new Error("missing complete Pi session header");
}

export function openSession(
  sdk: SessionSdk, cwd: string, sessionDir: string, transcript?: string, fresh = false,
): SessionManager {
  if (fresh || !transcript) return sdk.SessionManager.create(cwd, sessionDir);
  try {
    const fd = openSync(transcript, "r+");
    let id: string;
    try {
      const { size } = fstatSync(fd, { bigint: false });
      const bytes = Buffer.allocUnsafe(Math.min(size, 64 * 1024));
      const length = readSync(fd, bytes, 0, bytes.length, 0);
      const newline = bytes.subarray(0, length).indexOf(0x0a);
      if (newline < 0) throw new Error("missing complete Pi session header");
      const header = JSON.parse(bytes.toString("utf8", 0, newline));
      if (header?.type !== "session" || typeof header.id !== "string" || !header.id ||
        !Number.isSafeInteger(header.version) || header.version < 1 || header.version > sdk.CURRENT_SESSION_VERSION) {
        throw new Error("incompatible Pi session header");
      }
      id = header.id;
      discardIncompleteTail(fd, size);
    } finally { closeSync(fd); }
    const session = sdk.SessionManager.open(transcript, sessionDir, cwd);
    if (session.getSessionId() !== id) throw new Error("the recorded Pi session was not opened");
    return session;
  } catch (error) {
    throw new SessionResumeRefused(transcript, error);
  }
}

/** Assistant messages name the physical model, not necessarily the owner's selection. */
export function restoreModelChoice(session: SessionManager, interactive: boolean,
  fallbacks: readonly ModelChoice[] = [], legacyAnthropicApi = false): ModelChoice | undefined {
  const branch = session.getBranch();
  const saved = branch.findLast(entry => entry.type === "model_change");
  if (!saved || !isProviderId(saved.provider)) return undefined;
  if (legacyAnthropicApi && saved.provider === "anthropic") {
    // Old API transcripts used the upstream provider name. Record the disambiguated choice once.
    session.appendModelChange("anthropic-api", saved.modelId);
    return { provider: "anthropic-api", model: saved.modelId };
  }
  if (interactive || fallbacks.some(entry => entry.provider === saved.provider && entry.model === saved.modelId)) {
    return { provider: saved.provider, model: saved.modelId };
  }
  return undefined;
}
