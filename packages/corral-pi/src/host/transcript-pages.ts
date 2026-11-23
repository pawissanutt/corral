import type { FileHandle } from "node:fs/promises";
import type { TranscriptRow } from "./transcript-preview.ts";

export const BYTES = 24 * 1024;
const RECORDS = 32;
const WINDOW = 64 * 1024;

export class TranscriptBytes {
  private window: Buffer = Buffer.alloc(0);
  private start = 0;
  constructor(private readonly fd: FileHandle) {}

  async read(start: number, length: number): Promise<Buffer> {
    const buffer = Buffer.allocUnsafe(length);
    for (let done = 0; done < length;) {
      const { bytesRead } = await this.fd.read(buffer, done, length - done, start + done);
      if (!bytesRead) throw new Error("Transcript changed; load latest messages");
      done += bytesRead;
    }
    return buffer;
  }

  // Position just after the last newline in [floor, end).
  async boundary(end: number, floor: number): Promise<number> {
    while (end > floor) {
      if (end <= this.start || end > this.start + this.window.length) {
        this.start = Math.max(floor, end - WINDOW);
        this.window = await this.read(this.start, end - this.start);
      }
      const nl = this.window.lastIndexOf(10, end - this.start - 1);
      if (nl >= 0 && this.start + nl >= floor) return this.start + nl + 1;
      end = this.start;
    }
    return floor;
  }
}

export type BranchPosition = { end: number; parent?: string; skip?: number; offset?: number };
export type Position = number | BranchPosition;

type Item = Pick<TranscriptRow, "text" | "kind" | "callId">;
function project(entry: Record<string, unknown>): Item[] {
  const msg = entry.type === "message" ? entry.message as Record<string, unknown> | undefined : entry;
  if (entry.type === "compaction" || entry.type === "branch_summary") {
    return [{ kind: "note", text: `[${entry.type === "compaction" ? "Compaction" : "Branch summary"}] ${typeof entry.summary === "string" ? entry.summary : ""}` }];
  }
  if (!msg || (entry.type !== "message" && entry.type !== "custom_message" && entry.type !== "custom")) return [];
  const kind = msg.role === "user" ? "user" : msg.role === "assistant" ? "assistant" : msg.role === "toolResult" || msg.role === "tool" ? "tool" : "note";
  const callId = typeof msg.toolCallId === "string" ? msg.toolCallId : undefined;
  const content = msg.content;
  if (typeof content === "string") return [{ kind, text: content, callId }];
  if (!Array.isArray(content)) return [];
  return content.map((block): Item => {
    if (block?.type === "text" && typeof block.text === "string") return { kind, text: block.text, callId };
    if (block?.type === "thinking" && typeof block.thinking === "string") return { kind: "thinking", text: block.thinking };
    if (block?.type === "toolCall") return {
      kind: "tool", text: `${typeof block.name === "string" ? block.name : "Tool call"}: ${JSON.stringify(block.arguments ?? {})}`,
      callId: typeof block.id === "string" ? block.id : undefined,
    };
    return { kind, text: `[${typeof block?.type === "string" ? block.type.slice(0, 80) : "non-text"} attachment]`, callId };
  });
}

export async function conversationPage(bytes: TranscriptBytes, floor: number, position: BranchPosition) {
  let { end, parent, skip = 0, offset } = position;
  const rows: TranscriptRow[] = [];
  let remaining = BYTES, records = 0;
  while (end > floor && records < RECORDS && remaining > 0) {
    const start = await bytes.boundary(end - 1, floor);
    let entry: Record<string, unknown> | undefined;
    try {
      const parsed: unknown = JSON.parse((await bytes.read(start, end - start)).toString("utf8"));
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed) && "type" in parsed && typeof parsed.type === "string") entry = parsed as Record<string, unknown>;
    } catch (error) {
      if (!(error instanceof SyntaxError)) throw error;
    }
    // Skipped siblings cost a decode too; the cursor carries the sought parent across empty pages.
    records++;
    if (entry && parent !== undefined && parent !== entry.id) {
      end = start;
      continue;
    }
    const malformed = !entry || (entry.type === "message" && (!entry.message || typeof entry.message !== "object" || Array.isArray(entry.message)));
    const items = malformed ? [{ kind: "note" as const, text: `[Malformed transcript record at byte ${start}; inspect raw transcript]` }] : project(entry!);
    const date = typeof entry?.timestamp === "string" ? Date.parse(entry.timestamp) : Number.NaN;
    for (; skip < items.length; skip++, offset = undefined) {
      const index = items.length - 1 - skip, item = items[index]!;
      const stop = offset ?? item.text.length;
      let begin = stop, cost = 0;
      while (begin > 0) {
        let prev = begin - 1;
        const code = item.text.charCodeAt(prev);
        if (code >= 0xdc00 && code <= 0xdfff && prev > 0) {
          const high = item.text.charCodeAt(prev - 1);
          if (high >= 0xd800 && high <= 0xdbff) prev--;
        }
        const size = Buffer.byteLength(item.text.slice(prev, begin));
        if (cost + size > remaining) break;
        begin = prev;
        cost += size;
      }
      if (begin < stop || stop === 0) {
        rows.push({ ...item, id: `record:${start}:${index}:${begin}`, text: item.text.slice(begin, stop), at: Number.isFinite(date) ? date : undefined });
        remaining -= Math.max(1, cost);
      }
      if (begin > 0) return { rows: rows.reverse(), next: { end, parent, skip, offset: begin } };
      if (remaining <= 0 && skip + 1 < items.length) return { rows: rows.reverse(), next: { end, parent, skip: skip + 1 } };
    }
    if (entry?.parentId === null && typeof entry.id === "string") {
      end = floor;
      break;
    }
    if (typeof entry?.parentId === "string" && typeof entry.id === "string") parent = entry.parentId;
    end = start;
    skip = 0;
    offset = undefined;
  }
  return { rows: rows.reverse(), next: end > floor ? { end, parent } : undefined };
}

export async function rawPage(bytes: TranscriptBytes, end: number, start: number) {
  const data = await bytes.read(start, Math.min(end - start, BYTES + 1));
  let limit = Math.min(data.length, BYTES);
  while (limit > 0 && limit < data.length && (data[limit]! & 0xc0) === 0x80) limit--;
  // Invalid UTF-8 must not stall the reader.
  if (!limit && data.length) limit = Math.min(data.length, BYTES);
  const rows: TranscriptRow[] = [];
  let pos = 0;
  let continued = start > 0 && (await bytes.read(start - 1, 1))[0] !== 10;
  while (pos < limit && rows.length < RECORDS) {
    const newline = data.indexOf(10, pos);
    const complete = newline >= 0 && newline < limit;
    const stop = complete ? newline + 1 : limit;
    rows.push({ id: `raw:${start + pos}${continued || !complete ? ":continued" : ""}`, kind: "raw", text: data.toString("utf8", pos, stop) });
    continued = false;
    pos = stop;
  }
  return { rows, next: start + pos < end ? start + pos : undefined };
}
