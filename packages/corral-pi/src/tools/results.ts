// One machine contract for canonical responses and host failures. Never invent a receipt
// after a lost acknowledgement: only Rust can establish the operation's effect.
import { Type, type TSchema } from "typebox";

export function corralOutput(result: TSchema = Type.Unknown()) {
  const identity = {
    version: Type.Integer(),
    request_id: Type.Union([Type.String(), Type.Null()]),
    op: Type.String(),
    replayed: Type.Optional(Type.Boolean()),
    identity: Type.Optional(Type.Union([Type.Record(Type.String(), Type.Unknown()), Type.Null()])),
  };
  const canonical = { additionalProperties: true,
    description: "Canonical Corral response; all identity, refusal and continuation fields are retained" };
  return Type.Union([
  Type.Object({ ...identity, state: Type.Literal("committed"), result }, canonical),
  Type.Object({ ...identity,
    state: Type.Union(["rejected", "reserved", "queued", "uncertain", "abandoned"].map((s) => Type.Literal(s))),
    result: Type.Object({ code: Type.Optional(Type.String()), reason: Type.Optional(Type.String()) }, { additionalProperties: true }),
  }, canonical),
  Type.Object({
    request_id: Type.String(),
    op: Type.String(),
    error: Type.Object({
      code: Type.String(),
      category: Type.Union([Type.Literal("invalid_arguments"), Type.Literal("runtime_failure"), Type.Literal("uncertain_effect")]),
      reason: Type.String(),
      effect: Type.Union([Type.Literal("not_started"), Type.Literal("unknown")]),
      exit_code: Type.Optional(Type.Union([Type.Integer(), Type.Null()])),
      signal: Type.Optional(Type.Union([Type.String(), Type.Null()])),
      timed_out: Type.Optional(Type.Boolean()),
      diagnostic: Type.Optional(Type.String()),
      fields: Type.Optional(Type.Array(Type.Object({
        path: Type.String(), keyword: Type.String(), accepted: Type.Unknown(),
      }))),
      lookup: Type.Optional(Type.Object({
        tool: Type.Literal("corral_receipt_lookup"),
        arguments: Type.Object({ request_id: Type.String(), task: Type.Optional(Type.String()) }),
      })),
    }),
  }, { additionalProperties: false }),
  ]);
}
