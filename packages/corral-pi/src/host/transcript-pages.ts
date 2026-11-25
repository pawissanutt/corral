import { createHash } from "node:crypto";
import type { FileHandle } from "node:fs/promises";
import { toolMessageId } from "../extensions/subagent/inspection.ts";
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

// IDs can be arbitrarily large. Retain only a fixed-size key in checkpoints.
const idKey = (id: string) => createHash("sha256").update(id, "utf16le").digest("base64url");
export type BranchPosition = { end: number; parent?: string; skip?: number; offset?: number };
export type Position = number | BranchPosition;

type Item = Pick<TranscriptRow, "text" | "kind" | "callId" | "tool" | "error" | "stop">;
// Names and call ids sit outside the page's text budget and repeat on every slice, so they are bounded here. A call
// and its result carry the same fixed-size key of the call's id, the one its live projection has.
const toolName = (value: unknown) => typeof value === "string" && value ? value.slice(0, 200) : undefined;
const callKey = (value: unknown) => typeof value === "string" ? toolMessageId(value) : undefined;
function project(entry: Record<string, unknown>): Item[] {
  const msg = entry.type === "message" ? entry.message as Record<string, unknown> | undefined : entry;
  if (entry.type === "compaction" || entry.type === "branch_summary") {
    return [{ kind: "note", text: `[${entry.type === "compaction" ? "Compaction" : "Branch summary"}] ${typeof entry.summary === "string" ? entry.summary : ""}` }];
  }
  if (!msg || (entry.type !== "message" && entry.type !== "custom_message" && entry.type !== "custom")) return [];
  // The system prompt is the session's setup, not a turn of its conversation; raw mode still reads it.
  if (msg.role === "system") return [];
  const result = msg.role === "toolResult" || msg.role === "tool";
  const kind = msg.role === "user" ? "user" : msg.role === "assistant" ? "assistant" : result ? "tool" : "note";
  const base: Omit<Item, "text"> = result
    ? { kind, callId: callKey(msg.toolCallId), tool: toolName(msg.toolName), ...(msg.isError === true ? { error: true } : {}) }
    : { kind };
  const content = msg.content;
  const items = typeof content === "string" ? [{ ...base, text: content }] : !Array.isArray(content) ? [] : content.map((block): Item => {
    if (block?.type === "text" && typeof block.text === "string") return { ...base, text: block.text };
    if (block?.type === "thinking" && typeof block.thinking === "string") return { kind: "thinking", text: block.thinking };
    if (block?.type === "toolCall") return {
      kind: "call", tool: toolName(block.name) ?? "tool", text: JSON.stringify(block.arguments ?? {}),
      callId: callKey(block.id),
    };
    return { ...base, text: `[${typeof block?.type === "string" ? block.type.slice(0, 80) : "non-text"} attachment]` };
  });
  // A result without text still settles its call.
  if (result && !items.length) items.push({ ...base, text: "" });
  const stop = msg.role === "assistant" && (msg.stopReason === "error" || msg.stopReason === "aborted" || msg.stopReason === "length") ? msg.stopReason : undefined;
  return stop ? [...items, { kind: "stop", stop, text: typeof msg.errorMessage === "string" ? msg.errorMessage : "" }] : items;
}

/**
 * One page of the branch that ends at `position`, read back towards `floor`. `leaf` is the key of the record a page
 * that starts at a record's end began with; `parent` is the key of the record the branch continues at below what was
 * read, and `root` says the page reached the branch's first record.
 */
export async function conversationPage(bytes: TranscriptBytes, floor: number, position: BranchPosition) {
  let { end, parent, skip = 0, offset } = position;
  const rows: TranscriptRow[] = [];
  let remaining = BYTES, records = 0, met = false, root = false, leaf: string | undefined;
  const page = (next: BranchPosition | undefined) => ({ rows: rows.reverse(), next, leaf, parent, root });
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
    if (entry && parent !== undefined && (typeof entry.id !== "string" || parent !== idKey(entry.id))) {
      end = start;
      continue;
    }
    if (!met) {
      met = true;
      if (position.parent === undefined && typeof entry?.id === "string") leaf = idKey(entry.id);
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
        rows.push({ ...item, id: `record:${start}:${index}:${begin}`, text: item.text.slice(begin, stop), at: Number.isFinite(date) ? date : undefined,
          record: start, item: index, ...(begin > 0 ? { from: begin } : {}), ...(stop < item.text.length ? { more: true } : {}) });
        remaining -= Math.max(1, cost);
      }
      if (begin > 0) return page({ end, parent, skip, offset: begin });
      if (remaining <= 0 && skip + 1 < items.length) return page({ end, parent, skip: skip + 1 });
    }
    if (entry?.parentId === null && typeof entry.id === "string") {
      end = floor;
      root = true;
      break;
    }
    if (typeof entry?.parentId === "string" && typeof entry.id === "string") parent = idKey(entry.parentId);
    end = start;
    skip = 0;
    offset = undefined;
  }
  return page(end > floor ? { end, parent } : undefined);
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
