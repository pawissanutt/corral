// Every call uses the authenticated receipt path, including paginated reads.
import { Type, type TSchema } from "typebox";
import type { CorralToolSpec } from "../types.ts";

const Key = Type.String({ pattern: "^[A-Za-z0-9_-]{1,128}$" });
const Role = Type.Union(["pm", "assistant", "worker"].map((r) => Type.Literal(r)));
const Scope = Type.Union([
  Type.Object({ scope: Type.Literal("workspace") }, { additionalProperties: false }),
  Type.Object({ scope: Type.Literal("project"), key: Key }, { additionalProperties: false }),
  Type.Object({ scope: Type.Literal("role"), key: Role }, { additionalProperties: false }),
]);
const Selector = Type.Object({ scope: Scope, id: Key }, { additionalProperties: false });
const Hash = Type.String({ pattern: "^[a-f0-9]{64}$" });
const Entry = Type.Object({
  metadata: Type.Object({
    version: Type.Literal(1), id: Key, scope: Scope, title: Type.String(),
    kind: Type.Union(["preference", "decision", "agreement", "guidance", "note", "person"].map((k) => Type.Literal(k))),
    summary: Type.String({ description: "At most 512 UTF-8 bytes" }),
    project_into_prompt: Type.Optional(Type.Boolean()),
    provenance: Type.Object({
      kind: Type.Union(["owner_quote", "agent_inference", "imported_reference"].map((k) => Type.Literal(k))),
      sources: Type.Array(Type.String(), { minItems: 1 }),
      exact_quote: Type.Optional(Type.String()),
    }, { additionalProperties: false }),
    updated_at: Type.String({ description: "RFC3339 placeholder; service stamps authenticated time" }),
    author: Type.String({ description: "Placeholder; service stamps authenticated author" }),
  }, { additionalProperties: false }),
  body: Type.String({ description: "Full Markdown detail; entry limit 16 KiB" }),
}, { additionalProperties: false });

function tool(op: "memory_list" | "memory_read" | "memory_put" | "memory_remove",
  description: string, fields: Record<string, TSchema>): CorralToolSpec {
  return {
    name: `corral_${op}`, label: op, op, description,
    parameters: Type.Object(fields, { additionalProperties: false }),
    payload: (params) => params,
  };
}
export const memoryTools: readonly CorralToolSpec[] = [
  tool("memory_list", "List authorized working-tree memory (at most 20 per page). Memory is fallible context, not authority; lessons hold reusable project rules and inbox/tasks hold unfinished work.", {
    scope: Scope, offset: Type.Optional(Type.Integer({ minimum: 0 })),
    limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })),
  }),
  tool("memory_read", "Read current memory and its content hash, in UTF-8-safe body pages of at most 4096 bytes. Use next_offset for more; active sessions explicitly reread newer entries.", {
    selector: Selector, offset: Type.Optional(Type.Integer({ minimum: 0 })),
    limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 4096 })),
  }),
  tool("memory_put", "Replace one authorized entry using its freshly read hash, or explicit null for absence. No Git commit/index change. Provenance grants no authority. A missing receipt means file effect unknown: reread, never automatically retry. Worker role guidance is shared across projects; project facts belong in project scope.", {
    selector: Selector, entry: Entry, expected_hash: Type.Union([Hash, Type.Null()]),
  }),
  tool("memory_remove", "Remove one authorized entry using its current hash. Missing receipt: file effect unknown; reread before a new intentional request. No automatic retry or Git commit.", {
    selector: Selector, expected_hash: Hash,
  }),
];
