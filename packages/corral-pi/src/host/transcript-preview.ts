import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { constants, closeSync, fstatSync, lstatSync, openSync, readSync, type BigIntStats } from "node:fs";
import { lstat, open } from "node:fs/promises";
import { BYTES, TranscriptBytes, conversationPage, rawPage, type BranchPosition, type Position } from "./transcript-pages.ts";

export type TranscriptRequest = { mode: "conversation" | "raw"; cursor?: string };
export type TranscriptPage = { rows: readonly TranscriptRow[]; older?: string; newer?: string; stamp: string };
/**
 * One slice of a recorded item. A conversation row names its record (`record`, the record's byte offset) and its
 * item inside the record (`item`); `from` is where the slice starts in the item's text when the page budget split
 * it, and `more` says the item continues on a newer page. `call` is a tool call: `tool` names it and `text` is its
 * arguments as JSON. `tool` is a tool result, `error` when it failed. `stop` ends an assistant turn that failed, was
 * stopped or hit the output limit; its text is the recorded error message.
 */
export type TranscriptRow = {
  id: string; kind: "user" | "assistant" | "thinking" | "call" | "tool" | "stop" | "note" | "raw"; text: string; at?: number; callId?: string;
  tool?: string; error?: boolean; stop?: "error" | "aborted" | "length";
  record?: number; item?: number; from?: number; more?: boolean;
};

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

type Cursor = { stamp: string; mode: TranscriptRequest["mode"]; index: number; generation: string };
// Store resumable boundaries, never rows or record contents. Page indices
// include empty scan-only pages and are independent in each mode.
const MAX_SNAPSHOTS = 16;
type Checkpoints = { generation: string; end: number; raw: Position[]; conversation: Position[] };
const snapshots = new Map<string, Checkpoints>();
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
    const cached = snapshots.get(stamp);
    if (cursor && (!cached || cached.generation !== cursor.generation)) throw stale();
    const end = cached?.end ?? await bytes.boundary(size, first.end);
    if (!Number.isSafeInteger(end) || end < first.end || end > size) throw new Error("Invalid transcript cursor bounds");
    const initial = mode === "raw" ? 0 : { end };
    const read = (position: Position) => mode === "raw"
      ? rawPage(bytes, end, position as number)
      : conversationPage(bytes, first.end, position as BranchPosition);
    const index = cursor?.index ?? 0;
    const position = index === 0 ? initial : cached?.[mode][index];
    if (position === undefined) throw stale();
    const page = await read(position);
    await checkPath();
    if (snapshot(await fd.stat({ bigint: true })) !== sig) throw stale();
    // A concurrent request may have evicted this cursor's index or created
    // an index for a concurrent first-page read. Publish only after validation.
    let active = snapshots.get(stamp);
    if (cursor && active !== cached) throw stale();
    active ??= { end, generation: randomBytes(12).toString("base64url"), raw: [0], conversation: [{ end }] };
    if (page.next !== undefined) active[mode][index + 1] = page.next;
    snapshots.delete(stamp);
    snapshots.set(stamp, active);
    if (snapshots.size > MAX_SNAPSHOTS) snapshots.delete(snapshots.keys().next().value!);
    const link = (index: number) => encode({ stamp, mode, index, generation: active.generation });
    const next = page.next === undefined ? undefined : link(index + 1);
    const prev = index === 0 ? undefined : link(index - 1);
    return { rows: page.rows, stamp, older: mode === "raw" ? prev : next, newer: mode === "raw" ? next : prev };
  } finally { await fd.close(); }
}
