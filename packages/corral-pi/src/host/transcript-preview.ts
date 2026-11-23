import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { constants, closeSync, fstatSync, lstatSync, openSync, readSync, type BigIntStats } from "node:fs";
import { lstat, open } from "node:fs/promises";
import { BYTES, TranscriptBytes, conversationPage, rawPage, type BranchPosition, type Position } from "./transcript-pages.ts";

export type TranscriptRequest = { mode: "conversation" | "raw"; cursor?: string };
export type TranscriptPage = { rows: readonly TranscriptRow[]; older?: string; newer?: string; stamp: string };
export type TranscriptRow = { id: string; kind: "user" | "assistant" | "thinking" | "tool" | "note" | "raw"; text: string; at?: number; callId?: string };

const FLAGS = constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK;
const snapshot = (stat: BigIntStats) => [stat.dev, stat.ino, stat.size, stat.mtimeNs, stat.ctimeNs].join(":");
const stale = () => new Error("Transcript changed; load latest messages");

function header(buffer: Buffer) {
  const nl = buffer.indexOf(10);
  if (nl < 0) throw new Error("Transcript header missing or exceeds the header bound");
  const value = JSON.parse(buffer.toString("utf8", 0, nl));
  if (value?.type !== "session" || typeof value.id !== "string" || !value.id) throw new Error("Invalid transcript header");
  return { id: value.id as string, end: nl + 1 };
}

/** Small synchronous header probe for the existing synchronous app path resolver. */
export function readTranscriptHeader(file: string): string | undefined {
  try {
    const fd = openSync(file, FLAGS);
    try {
      const stat = fstatSync(fd, { bigint: true });
      if (!stat.isFile()) return undefined;
      const buffer = Buffer.alloc(BYTES);
      let count = 0;
      while (count < buffer.length) {
        const n = readSync(fd, buffer, count, buffer.length - count, count);
        if (!n) break;
        count += n;
        if (buffer.subarray(0, count).includes(10)) break;
      }
      const result = header(buffer.subarray(0, count));
      const sig = snapshot(stat);
      const pathStat = lstatSync(file, { bigint: true });
      if (pathStat.isFile() && snapshot(pathStat) === sig && snapshot(fstatSync(fd, { bigint: true })) === sig) return result.id;
    } finally { closeSync(fd); }
  } catch { /* Missing or unsafe recording. */ }
  return undefined;
}

type Cursor = { stamp: string; mode: TranscriptRequest["mode"]; index: number; end: number; position?: Position; previous?: Position };
const key = randomBytes(32);
const mac = (body: string) => createHmac("sha256", key).update(body).digest();
function encode(cursor: Cursor) {
  const body = Buffer.from(JSON.stringify(cursor)).toString("base64url");
  return `${body}.${mac(body).toString("base64url")}`;
}
function decode(value: string): Cursor {
  const [body, sig, extra] = value.split(".");
  if (!body || !sig || extra !== undefined || sig.length !== 43) throw new Error("Invalid transcript cursor");
  const signature = Buffer.from(sig, "base64url");
  if (signature.length !== 32 || !timingSafeEqual(mac(body), signature) || signature.toString("base64url") !== sig) throw new Error("Invalid transcript cursor");
  return JSON.parse(Buffer.from(body, "base64url").toString("utf8"));
}

/** Never open through SessionManager: inspecting an old session must not migrate it. */
export async function readTranscriptPage(file: string, session: string, request: TranscriptRequest): Promise<TranscriptPage> {
  const { mode } = request;
  if (mode !== "conversation" && mode !== "raw") throw new Error("Invalid transcript mode");
  const cursor = request.cursor === undefined ? undefined : decode(request.cursor);
  if (cursor && cursor.mode !== mode) throw new Error("Invalid transcript cursor mode");
  const fd = await open(file, FLAGS);
  try {
    const stat = await fd.stat({ bigint: true });
    if (!stat.isFile()) throw new Error("Transcript is not an ordinary regular file");
    const size = Number(stat.size);
    if (!Number.isSafeInteger(size)) throw new Error("Transcript is too large to address safely");
    const sig = snapshot(stat);
    const stamp = createHash("sha256").update(JSON.stringify([file, session, sig])).digest("base64url");
    if (cursor && cursor.stamp !== stamp) throw stale();
    async function checkPath() {
      const pathStat = await lstat(file, { bigint: true }).catch(() => undefined);
      if (!pathStat?.isFile() || snapshot(pathStat) !== sig) throw stale();
    }
    await checkPath();
    const bytes = new TranscriptBytes(fd);
    const first = header(await bytes.read(0, Math.min(BYTES, size)));
    if (first.id !== session) throw new Error("Transcript identity mismatch");
    const end = cursor?.end ?? await bytes.boundary(size, first.end);
    if (!Number.isSafeInteger(end) || end < first.end || end > size) throw new Error("Invalid transcript cursor bounds");
    const initial = mode === "raw" ? 0 : { end };
    const read = (position: Position) => mode === "raw"
      ? rawPage(bytes, end, position as number)
      : conversationPage(bytes, first.end, position as BranchPosition);
    const index = cursor?.index ?? 0;
    let position = cursor?.position ?? initial;
    const offset = typeof position === "number" ? position : position.end;
    if (!Number.isSafeInteger(offset) || offset < (mode === "raw" ? 0 : first.end) || offset > end) throw new Error("Invalid transcript cursor bounds");
    // The preceding boundary is carried for immediate reversal. With no
    // checkpoint, replay deterministically without retaining an unbounded index.
    let previous = cursor?.previous;
    for (let i = 0; cursor?.position === undefined && i < index; i++) {
      previous = position;
      const { next } = await read(position);
      if (next === undefined) throw new Error("Invalid transcript cursor bounds");
      position = next;
    }
    const page = await read(position);
    await checkPath();
    if (snapshot(await fd.stat({ bigint: true })) !== sig) throw stale();
    const next = page.next === undefined ? undefined : encode({ stamp, mode, index: index + 1, end, position: page.next, previous: position });
    const prev = index === 0 ? undefined : encode({ stamp, mode, index: index - 1, end, position: previous });
    return { rows: page.rows, stamp, older: mode === "raw" ? prev : next, newer: mode === "raw" ? next : prev };
  } finally { await fd.close(); }
}
