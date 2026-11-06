import { closeSync, existsSync, openSync, readSync } from "node:fs";

import { JobError } from "./runtime.ts";

export interface OutputSource {
  readonly id: string; readonly file: string; readonly bytes: number; readonly stored: number; readonly tail: Buffer;
  readonly state?: "running" | "exited";
}
export interface OutputOptions { readonly offset?: number; readonly cursor?: string; readonly max_bytes?: number; readonly tail_lines?: number }

/** A peek never advances shared state. Offsets count observed bytes, including explicit retention gaps. */
export function readOutput(source: OutputSource, options: OutputOptions): Record<string, unknown> {
  const { id, file, bytes, stored, tail } = source;
  const max = Math.min(Math.max(1, options.max_bytes ?? 16384), 65536);
  if (options.cursor !== undefined && (options.offset !== undefined || options.tail_lines !== undefined)) {
    throw new JobError("cursor cannot be combined with offset or tail_lines");
  }
  if (options.offset !== undefined && options.tail_lines !== undefined) throw new JobError("offset cannot be combined with tail_lines");
  let requested = options.offset;
  if (options.cursor !== undefined) {
    const prefix = `${id}:`;
    const value = options.cursor.slice(prefix.length);
    if (!options.cursor.startsWith(prefix) || !/^\d+$/.test(value) || !Number.isSafeInteger(Number(value))) {
      throw new JobError("invalid or foreign output cursor");
    }
    requested = Number(value);
  }
  const tailStart = bytes - tail.length;
  let from = requested ?? Math.max(tailStart, bytes - max);
  if (!Number.isSafeInteger(from) || from < 0 || from > bytes) throw new JobError("output offset is outside this job's observed output");
  let gap: { from: number; to: number } | undefined;
  let buffer: Buffer;
  let missing = !existsSync(file);
  if (requested !== undefined && from < stored) {
    let fd: number | undefined;
    try {
      fd = openSync(file, "r");
      buffer = Buffer.alloc(Math.min(max, stored - from));
      buffer = buffer.subarray(0, readSync(fd, buffer, 0, buffer.length, from));
    } catch {
      missing = true;
      gap = { from, to: tailStart };
      from = tailStart;
      buffer = tail.subarray(0, max);
    } finally {
      if (fd !== undefined) closeSync(fd);
    }
  } else {
    if (from < tailStart) { gap = { from, to: tailStart }; from = tailStart; }
    buffer = tail.subarray(from - tailStart, from - tailStart + max);
  }
  // Never manufacture U+FFFD by splitting a multibyte sequence at a page boundary.
  let begin = 0;
  while (begin < buffer.length && (buffer[begin]! & 0xc0) === 0x80) begin++;
  let end = buffer.length;
  let lead = end - 1;
  while (lead >= begin && (buffer[lead]! & 0xc0) === 0x80) lead--;
  if (lead >= begin) {
    const byte = buffer[lead]!;
    const length = byte >= 0xc2 && byte <= 0xdf ? 2 : byte >= 0xe0 && byte <= 0xef ? 3 : byte >= 0xf0 && byte <= 0xf4 ? 4 : 1;
    if (lead + length > end && !(source.state === "exited" && from + end === bytes)) end = lead;
  }
  // A capped prefix can end in the middle of a code point. Do not strand a cursor there.
  if (end === begin && buffer.length && from + buffer.length === stored && stored < tailStart) {
    return { ...readOutput(source, { offset: tailStart, max_bytes: max }), gap: { from, to: tailStart } };
  }
  const tailLines = options.tail_lines ?? (requested === undefined ? 50 : undefined);
  if (tailLines !== undefined) {
    let lines = 0;
    for (let i = end - 2; i >= begin; i--) {
      if (buffer[i] === 10 && ++lines === tailLines) { begin = i + 1; break; }
    }
  }
  const next = from + end;
  from += begin;
  const text = buffer.subarray(begin, end).toString("utf8");
  return { offset: from, next_offset: next, cursor: `${id}:${next}`, text, has_more: next < bytes,
    ...(gap ? { gap } : {}), ...(missing ? { output_file: "missing" } : {}),
    ...(end === begin && buffer.length && max < 4 ? { minimum_page_bytes: 4 } : {}) };
}
