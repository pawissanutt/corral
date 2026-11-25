import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { constants, closeSync, fstatSync, lstatSync, openSync, readSync, type BigIntStats } from "node:fs";
import { lstat, open } from "node:fs/promises";
import { BYTES, TranscriptBytes, conversationPage, rawPage, type BranchPosition, type Position } from "./transcript-pages.ts";

export type TranscriptRequest = { mode: "conversation" | "raw"; cursor?: string };
/**
 * `grow` comes with the page at a snapshot's end and with every page read through it: a cursor that reads what was
 * recorded after that end, in order, as one more page of the same snapshot.
 */
export type TranscriptPage = { rows: readonly TranscriptRow[]; older?: string; newer?: string; stamp: string; grow?: string };
/**
 * One slice of a recorded item. A conversation row names its record (`record`, the record's byte offset) and its
 * item inside the record (`item`); `from` is where the slice starts in the item's text when the page budget split
 * it, and `more` says the item continues on a newer page. `call` is a tool call: `tool` names it and `text` is its
 * arguments as JSON. `tool` is a tool result, `error` when it failed. A call and its result share `callId`, a
 * fixed-size key of the call's id (`toolMessageId`). `stop` ends an assistant turn that failed, was stopped or hit
 * the output limit; its text is the recorded error message.
 */
export type TranscriptRow = {
  id: string; kind: "user" | "assistant" | "thinking" | "call" | "tool" | "stop" | "note" | "raw"; text: string; at?: number; callId?: string;
  tool?: string; error?: boolean; stop?: "error" | "aborted" | "length";
  record?: number; item?: number; from?: number; more?: boolean;
};

const FLAGS = constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK;
const snapshot = (stat: BigIntStats) => [stat.dev, stat.ino, stat.size, stat.mtimeNs, stat.ctimeNs].join(":");
const identity = (stat: BigIntStats) => `${stat.dev}:${stat.ino}`;
/** The most pages of new records one growth read gathers; a recording that grew by more is read again from its end. */
const GROW_PAGES = 8;
/** Times a growth read is made when the file is written while it is read; after that the recording is read from its end. */
const GROW_READS = 3;
/** Bytes before a snapshot's end that must read the same for its cursors to go on reading a file that changed. */
const PROBE = 4096;
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

/** `grow`: read what follows byte `tip`, where the branch ended at the record with key `leaf`. */
type Cursor = { stamp: string; mode: TranscriptRequest["mode"]; index: number; generation: string; grow?: { tip: number; leaf?: string; probe?: string } };
// Store resumable boundaries, never rows or record contents. Page indices
// include empty scan-only pages and are independent in each mode.
const MAX_SNAPSHOTS = 16;
/** `owner` is the file a snapshot was taken of and `probe` a digest of the bytes before its end. */
type Checkpoints = { generation: string; end: number; raw: Position[]; conversation: Position[]; owner: string; probe: string };
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

/**
 * Never open through SessionManager: inspecting an old session must not migrate it.
 *
 * A session only appends to its recording, so a snapshot's cursors go on reading while the file is the same one, no
 * shorter, and unchanged before the snapshot's end: an agent that is still writing never makes its reader start
 * over. A read without a cursor takes a new snapshot of the file as it is.
 */
export async function readTranscriptPage(file: string, session: string, request: TranscriptRequest, attempt = 0): Promise<TranscriptPage> {
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
    const taken = createHash("sha256").update(JSON.stringify([file, session, sig])).digest("base64url");
    const stamp = cursor?.stamp ?? taken, owner = JSON.stringify([file, session, identity(stat)]);
    const cached = snapshots.get(stamp);
    if (cursor && (!cached || cached.generation !== cursor.generation || cached.owner !== owner || size < cached.end)) throw stale();
    // A cursor reads bytes its snapshot already bounded, so the file may grow under it; a new snapshot is of a file at rest.
    const same = (now: BigIntStats | undefined) => now?.isFile() === true
      && (cursor ? identity(now) === identity(stat) && now.size >= stat.size : snapshot(now) === sig);
    async function checkPath() {
      if (!same(await lstat(file, { bigint: true }).catch(() => undefined))) throw stale();
    }
    await checkPath();
    const bytes = new TranscriptBytes(fd);
    const first = header(await bytes.read(0, Math.min(BYTES, size)));
    if (first.id !== session) throw new Error("Transcript identity mismatch");
    const end = cached?.end ?? await bytes.boundary(size, first.end);
    if (!Number.isSafeInteger(end) || end < first.end || end > size) throw new Error("Invalid transcript cursor bounds");
    // The bytes before a point a cursor ends at, at most PROBE of them: they must read the same for it to go on.
    const digest = async (to: number) => {
      const from = Math.max(first.end, to - PROBE);
      return createHash("sha256").update(await bytes.read(from, to - from)).digest("base64url");
    };
    const probe = stamp === taken && cached ? cached.probe : await digest(end);
    if (cached && cached.probe !== probe) throw stale();
    // The byte a conversation page must end on a line break at, once the page is known.
    let edge: number | undefined;
    const ended = async () => edge === undefined || (await bytes.read(edge, 1))[0] === 10;
    const settle = async () => {
      await checkPath();
      const after = await fd.stat({ bigint: true });
      if (!same(after)) throw stale();
      // Written while it was read: the bytes the snapshot ends on must still be the ones it was taken with.
      if (cursor && snapshot(after) !== sig && ((await digest(end)) !== probe || !(await ended()))) throw stale();
    };
    const growth = cursor?.grow;
    if (growth) {
      if (mode !== "conversation" || !cached) throw new Error("Invalid transcript cursor mode");
      const { tip, leaf } = growth, now = await bytes.boundary(size, first.end);
      if (!Number.isSafeInteger(tip) || tip < end || tip > now) throw stale();
      // What an earlier growth read handed over ends at the tip: those bytes must still be the ones it read.
      if (tip > end && (await digest(tip)) !== growth.probe) throw stale();
      // Read back from the new end to where the reader's pages stop, then handed over oldest first.
      const pages: (readonly TranscriptRow[])[] = [];
      let position: BranchPosition | undefined = tip < now ? { end: now } : undefined, parent: string | undefined, head: string | undefined;
      while (position) {
        if (pages.length === GROW_PAGES) throw stale();
        const page = await conversationPage(bytes, tip, position);
        if (!pages.length) head = page.leaf;
        // A branch that starts again, or continues from another record than the one the reader's pages end at, is another conversation.
        if (page.root) throw stale();
        pages.unshift(page.rows); position = page.next; parent = page.parent;
      }
      // What was added must be shown to continue from the record the reader's pages end at. A snapshot that ended
      // on a record without an id shows no such thing, and is read again from its end.
      if (tip < now && (leaf === undefined || parent !== leaf)) throw stale();
      const reached = tip < now ? head : leaf, ends = now > end ? await digest(now) : undefined;
      await checkPath();
      // The rows decoded and the bytes the next cursor ends on must be the same ones: a file written while it was
      // read is read again, and one written during every read is left to a read from its end.
      if (snapshot(await fd.stat({ bigint: true })) !== sig) {
        if (attempt + 1 >= GROW_READS) throw stale();
        return await readTranscriptPage(file, session, request, attempt + 1);
      }
      if (snapshots.get(stamp) !== cached) throw stale();
      snapshots.delete(stamp);
      snapshots.set(stamp, cached);
      return { rows: pages.flat(), stamp, grow: encode({ ...cursor,
        grow: { tip: now, ...(reached === undefined ? {} : { leaf: reached }), ...(ends === undefined ? {} : { probe: ends }) } }) };
    }
    const initial = mode === "raw" ? 0 : { end };
    const read = (position: Position) => mode === "raw"
      ? rawPage(bytes, end, position as number)
      : conversationPage(bytes, first.end, position as BranchPosition);
    const index = cursor?.index ?? 0;
    const position = index === 0 ? initial : cached?.[mode][index];
    if (position === undefined) throw stale();
    // A page of the conversation ends where a record ends: a file written over rarely keeps its line breaks in place.
    if (cursor && typeof position !== "number" && position.end > first.end) edge = position.end - 1;
    if (!(await ended())) throw stale();
    const page = await read(position);
    await settle();
    // A concurrent request may have evicted this cursor's index or created
    // an index for a concurrent first-page read. Publish only after validation.
    let active = snapshots.get(stamp);
    if (cursor && active !== cached) throw stale();
    active ??= { end, generation: randomBytes(12).toString("base64url"), raw: [0], conversation: [{ end }], owner, probe };
    if (page.next !== undefined) active[mode][index + 1] = page.next;
    snapshots.delete(stamp);
    snapshots.set(stamp, active);
    if (snapshots.size > MAX_SNAPSHOTS) snapshots.delete(snapshots.keys().next().value!);
    const link = (index: number) => encode({ stamp, mode, index, generation: active.generation });
    const next = page.next === undefined ? undefined : link(index + 1);
    const prev = index === 0 ? undefined : link(index - 1);
    const leaf = "leaf" in page ? page.leaf : undefined;
    const grow = mode === "conversation" && index === 0
      ? encode({ stamp, mode, index, generation: active.generation, grow: { tip: end, ...(leaf === undefined ? {} : { leaf }) } }) : undefined;
    return { rows: page.rows, stamp, older: mode === "raw" ? prev : next, newer: mode === "raw" ? next : prev, ...(grow === undefined ? {} : { grow }) };
  } finally { await fd.close(); }
}
