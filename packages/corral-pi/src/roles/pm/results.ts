// Bounded read results are objects, never JSON-encoded receipts. content_json itself
// intentionally remains a paged JSON text chunk: it may end in the middle of a value.
import { Type, type TSchema } from "typebox";
import type { CorralOp } from "../types.ts";

const Offset = Type.Union([Type.Integer({ minimum: 0 }), Type.Null()]);
const ObjectValue = Type.Record(Type.String(), Type.Unknown());
const Page = Type.Object({
  items: Type.Array(Type.Unknown()), total: Type.Integer({ minimum: 0 }), next_offset: Offset,
}, { additionalProperties: true });
const Content = {
  content_json: Type.String(), total_chars: Type.Integer({ minimum: 0 }), next_offset: Offset,
};
const Show = Type.Object({
  ...Content, task: Type.Optional(ObjectValue), section: Type.Optional(Type.String()),
  key: Type.Optional(Type.Union([Type.String(), Type.Null()])),
  revision: Type.Optional(Type.Union([Type.Integer(), Type.Null()])),
  offset: Type.Optional(Type.Integer({ minimum: 0 })), end_offset: Type.Optional(Type.Integer({ minimum: 0 })),
  truncated: Type.Optional(Type.Boolean()),
  diagnostic: Type.Optional(Type.Union([Type.String(), Type.Null()])),
}, { additionalProperties: true });

export function pmResultSchema(op: CorralOp): TSchema | undefined {
  if (op === "read_show") return Show;
  if (op === "inbox_read") return Type.Union([Page,
    Type.Object({ ...Content, item: ObjectValue, outcome: Type.Union([Type.String(), Type.Null()]) }, { additionalProperties: true }),
  ]);
  if (["read_list", "read_search", "read_artifacts", "read_lessons"].includes(op)) return Page;
  return undefined;
}
