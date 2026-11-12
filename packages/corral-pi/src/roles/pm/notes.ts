import { Type } from "typebox";
import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

const Fields = {
  text: Type.String(),
  provenance: Type.Optional(Type.String()),
  reason: Type.Optional(Type.String()),
  resume_when: Type.Optional(Type.String()),
  expected_revision: Type.Optional(Type.Integer({ minimum: 0 })),
};
export const notesTool: CorralToolSpec = {
  name: "corral_notes",
  label: "Preserve PM context",
  description: "Keep bounded notes as context for the PM itself across compaction and relaunch, never authority. "
    + "Owner instructions need provenance (a question key, message ID or event reference). "
    + "Deferrals require reason and resume_when. You cannot remove or supersede owner-authored notes.",
  op: "notes",
  parameters: Type.Union([
    Type.Object({ op: Type.Literal("list") }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("add"), kind: Type.Union([
      Type.Literal("instruction"), Type.Literal("deferral"), Type.Literal("hypothesis"), Type.Literal("commitment"),
    ]), ...Fields }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("supersede"), id: Type.String(), ...Fields }, { additionalProperties: false }),
    Type.Object({ op: Type.Literal("remove"), id: Type.String(), expected_revision: Fields.expected_revision }, { additionalProperties: false }),
  ]),
  payload(params) {
    return pick(params, ["op", "id", "kind", "text", "provenance", "reason", "resume_when", "expected_revision"]);
  },
};
