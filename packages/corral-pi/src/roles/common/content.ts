import { Type } from "typebox";
import type { CorralToolSpec } from "../types.ts";
import { pick } from "./tools.ts";

export const contentReadTool: CorralToolSpec = {
  name: "corral_content_read",
  label: "Read stored task content",
  description: "Read durable content without filesystem caches or a shell. Reference: T-123 for the task spec "
    + "and plan/artifact manifests (including review notes); T-123@P1 for an immutable plan snapshot; "
    + "artifacts/T-123/R1/file.md or T-123:R1:file.md for an artifact. Only tasks in your bound project are "
    + "readable. Text pages are at most 4096 UTF-8 bytes; continue at next_offset until it is null. "
    + "Use this instead of read when a prompt's .corral snapshot or artifact path is absent.",
  op: "read_content",
  parameters: Type.Object({
    reference: Type.String(),
    offset: Type.Optional(Type.Integer({ minimum: 0 })),
    limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 4096 })),
  }),
  payload: (params: Record<string, unknown>) => pick(params, ["reference", "offset", "limit"]),
};
