// Filing cites a server-journaled owner input. Rust preserves its verbatim text and origin,
// checks live device authority, and deduplicates model filing against file-as-written.
import { Type } from "typebox";
import type { CorralToolSpec } from "../types.ts";

export const ideaFileTool: CorralToolSpec = {
  name: "corral_idea_file", label: "File owner idea", op: "idea_file",
  description: "File the owner's idea as a real Corral task, citing the [owner input A-n/gN/iN] ID from their message. "
    + "Use title/body/labels/criteria only to structure that idea; Corral keeps the owner's original words verbatim, "
    + "records assistant origin and prevents duplicate filing of the same input. The owner's current project is the default; "
    + "set project only when the owner asks for a different configured project. Never cite a wake, context excerpt or tool answer.",
  parameters: Type.Object({
    input: Type.String({ minLength: 1 }), title: Type.String({ minLength: 1, maxLength: 200 }),
    body: Type.String({ maxLength: 16384 }),
    project: Type.Optional(Type.String({ minLength: 1, maxLength: 160 })),
    labels: Type.Optional(Type.Array(Type.String({ minLength: 1, maxLength: 64 }), { maxItems: 16 })),
    criteria: Type.Optional(Type.Array(Type.String({ minLength: 1, maxLength: 1024 }), { maxItems: 32 })),
  }, { additionalProperties: false }),
  payload: params => params,
};
