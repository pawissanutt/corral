import { constants, closeSync, fstatSync, openSync, readSync } from "node:fs";

const MAX_BYTES = 24 * 1024;
/** Never SessionManager.open(): that seam can migrate/rewrite a historical session. */
export function transcriptPreview(file: string, session: string): string {
  const fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!fstatSync(fd).isFile()) throw new Error("Transcript is not an ordinary regular file");
    const buffer = Buffer.alloc(MAX_BYTES + 1);
    const bytes = readSync(fd, buffer, 0, buffer.length, 0);
    const text = buffer.subarray(0, Math.min(bytes, MAX_BYTES)).toString("utf8");
    const first = text.indexOf("\n");
    if (first < 0) throw new Error("Transcript header missing or exceeds preview bound");
    const header = JSON.parse(text.slice(0, first));
    if (header.type !== "session" || header.id !== session) throw new Error("Transcript identity mismatch");
    const lines = text.split("\n");
    // A partial last record is never presented as a complete JSONL entry.
    if (bytes > MAX_BYTES || !text.endsWith("\n")) lines.pop();
    return ["Existing JSONL · first 200 lines / 24 KiB maximum · read-only snapshot", ...lines.slice(0, 200),
      bytes > MAX_BYTES || lines.length > 200 ? "[preview truncated]" : "[end of captured preview]"].join("\n");
  } finally { closeSync(fd); }
}
