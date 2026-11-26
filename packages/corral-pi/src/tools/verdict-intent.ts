import { randomUUID } from "node:crypto";
import { closeSync, constants as f, fchmodSync, fstatSync, fsyncSync, linkSync, lstatSync, openSync, readSync,
  renameSync, unlinkSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

export const REVIEW_REQUEST_PATTERN = "^[0-9]{1,20}-[0-9]{1,10}-(?:[0-9a-f]{40}|[0-9a-f]{64})$";
const REQUEST = new RegExp(REVIEW_REQUEST_PATTERN);
const PAYLOAD_MAX = 256 * 1024;
const ERROR_MAX = 4096;
const READ = f.O_RDONLY | f.O_NOFOLLOW | f.O_NONBLOCK;

function existingMatches(path: string, verdict: string, notes: string): boolean {
  const fd = openSync(path, READ);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.uid !== process.getuid?.() || (stat.mode & 0o777) !== 0o600 || stat.size > PAYLOAD_MAX) return false;
    const bytes = Buffer.alloc(stat.size + 1);
    let length = 0;
    while (length < bytes.length) {
      const count = readSync(fd, bytes, length, bytes.length - length, null);
      if (!count) break;
      length += count;
    }
    if (length !== stat.size) return false;
    const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, length)));
    return !!value && typeof value === "object" && Object.keys(value).length === 2 &&
      "verdict" in value && value.verdict === verdict && "notes" in value && value.notes === notes;
  } finally { closeSync(fd); }
}

function boundedError(text: string): string {
  let length = 0;
  let result = "";
  for (const part of text) {
    const clean = /[\x00-\x08\x0b-\x1f\x7f-\x9f]/.test(part) ? " " : part;
    length += Buffer.byteLength(clean);
    if (length > ERROR_MAX) break;
    result += clean;
  }
  return result;
}

/** Publish immutable intent before the engine can read it. Keep the run directory open for diagnostics. */
export function publishVerdictIntent(sessionDir: string, run: string, request: string, verdict: string, notes: string): {
  recordError(text: string): void; close(): void;
} {
  if (!/^R[0-9]{1,10}$/.test(run)) throw new Error("Verdict requires a valid reviewer run ID.");
  if (typeof request !== "string" || !REQUEST.test(request)) throw new Error("Copy the exact request ID from the prompt for the reviewed candidate.");
  if ((verdict !== "approve" && verdict !== "changes") || typeof notes !== "string") throw new Error("Invalid verdict intent.");
  if (notes.length > PAYLOAD_MAX) throw new Error("Verdict with notes exceeds 256 KiB of serialized UTF-8.");
  if (!notes.isWellFormed()) throw new Error("Verdict notes must be valid Unicode text.");
  const payload = JSON.stringify({ verdict, notes });
  if (Buffer.byteLength(payload) > PAYLOAD_MAX) throw new Error("Verdict with notes exceeds 256 KiB of serialized UTF-8.");

  const dir = resolve(sessionDir);
  const fd = openSync(dir, READ | f.O_DIRECTORY);
  try {
    const stat = fstatSync(fd, { bigint: true });
    const checkDirectory = () => {
      const current = lstatSync(dir, { bigint: true });
      if (current.dev !== stat.dev || current.ino !== stat.ino || !current.isDirectory() ||
        Number(current.uid) !== process.getuid?.() || (current.mode & 0o777n) !== 0o700n) {
        throw new Error("Verdict requires the original private (0700) reviewer run directory.");
      }
    };
    // Linux anchors writes to the open directory even across a rename. On other Unix hosts
    // use the engine-supplied private path, checking its identity around publication.
    const base = process.platform === "linux" ? `/proc/self/fd/${fd}` : dir;
    const publish = (extension: "json" | "error", data: string) => {
      checkDirectory();
      // The transcript directory is stable across reviewer replacement.
      const path = join(base, `verdict-${run}-${request}.${extension}`);
      const temp = join(base, `.verdict-${randomUUID()}.tmp`);
      const stage = openSync(temp, f.O_WRONLY | f.O_CREAT | f.O_EXCL | f.O_NOFOLLOW, 0o600);
      try {
        fchmodSync(stage, 0o600);
        writeFileSync(stage, data);
        fsyncSync(stage);
        checkDirectory();
        if (extension === "error") {
          renameSync(temp, path); // Replace the entry, never follow it.
        } else {
          try { linkSync(temp, path); }
          catch (error) {
            if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
            let matches = false;
            try { matches = existingMatches(path, verdict, notes); }
            catch { /* Unreadable or invalid entries also must not be replaced. */ }
            if (!matches) throw new Error(`A different or unreadable verdict is already saved for request ${request}; it was not replaced.`);
          }
        }
      } finally {
        try { closeSync(stage); }
        finally {
          try { unlinkSync(temp); }
          catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
        }
      }
      fsyncSync(fd);
      checkDirectory();
    };
    publish("json", payload);
    return { recordError: (text) => publish("error", boundedError(text)), close: () => closeSync(fd) };
  } catch (error) {
    closeSync(fd);
    throw error;
  }
}
