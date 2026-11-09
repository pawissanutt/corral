// Fixed, bounded immutable packet reader. No caller paths, shell, clients or child delegation.
import { openSync, fstatSync, readSync, closeSync } from "node:fs";
import { createHash } from "node:crypto";
import { Type } from "typebox";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { ReviewMission, ReviewSnapshot } from "./runtime.ts";

function snapshot(ref: ReviewSnapshot): Buffer {
  if (!Number.isSafeInteger(ref.bytes) || ref.bytes < 1 || ref.bytes > 1024 * 1024) throw new Error("invalid review snapshot size");
  const fd = openSync(ref.path, "r");
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size !== ref.bytes) throw new Error("review snapshot is not the expected bounded regular file");
    const buffer = Buffer.alloc(ref.bytes + 1);
    let read = 0;
    while (read < buffer.length) {
      const n = readSync(fd, buffer, read, buffer.length - read, null);
      if (!n) break;
      read += n;
    }
    const bytes = buffer.subarray(0, read);
    if (read !== ref.bytes || createHash("sha256").update(bytes).digest("hex") !== ref.digest) throw new Error("review snapshot digest differs");
    // Reject invalid UTF-8 rather than replacing bytes while claiming an exact review.
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return bytes;
  } finally { closeSync(fd); }
}

export function reviewSnapshots(mission: ReviewMission) {
  const data = { plan: snapshot(mission.plan), context: snapshot(mission.context) };
  const consumed = { plan: 0, context: 0 };
  const tool: ToolDefinition = {
    name: "read_review_snapshot", label: "Immutable review snapshot",
    description: "Read complete exact plan and original request/decisions/source context. Read sequential UTF-8 byte pages until next_offset is null for BOTH documents. No caller-selected filesystem path.",
    parameters: Type.Object({ document: Type.Union([Type.Literal("plan"), Type.Literal("context")]),
      offset: Type.Integer({ minimum: 0 }), limit: Type.Optional(Type.Integer({ minimum: 4, maximum: 16384 })) }, { additionalProperties: false }),
    async execute(_id, raw) {
      const input = raw as { document: "plan" | "context"; offset: number; limit?: number };
      const bytes = data[input.document];
      const start = input.offset;
      if (start > consumed[input.document] || start > bytes.length || (start < bytes.length && (bytes[start]! & 0xc0) === 0x80)) throw new Error("read review snapshots sequentially at UTF-8 boundaries");
      let end = Math.min(bytes.length, start + (input.limit ?? 16384));
      while (end < bytes.length && (bytes[end]! & 0xc0) === 0x80) end--;
      consumed[input.document] = Math.max(consumed[input.document], end);
      const result = { document: input.document, offset: start, text: bytes.subarray(start, end).toString("utf8"), next_offset: end < bytes.length ? end : null };
      return { content: [{ type: "text", text: JSON.stringify(result) }], details: result };
    },
  };
  return { tool, complete: () => consumed.plan === data.plan.length && consumed.context === data.context.length };
}
