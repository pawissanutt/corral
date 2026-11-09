// Assistant workflow protocol foundation. Intentionally NOT installed in the ordinary role
// loadout and no /plan command: the execution boundary and choice experience must land first.
// Model-callable schemas never contain host-only drain/result/answer publication actions.
import { Type } from "typebox";
import type { CorralOp, CorralToolSpec } from "../types.ts";

export const ASSISTANT_WORKFLOW_OPS: readonly CorralOp[] = [
  "assistant_plan", "assistant_plan_review", "assistant_handoff",
];
export const DESIGN_DOCUMENT_MAX_BYTES = 1024 * 1024;
export const DESIGN_CHUNK_MAX_BYTES = 16 * 1024;

const closed = { additionalProperties: false } as const;
const natural = Type.Integer({ minimum: 0 });
const positive = Type.Integer({ minimum: 1 });
const digest = Type.String({ pattern: "^[a-f0-9]{64}$" });
const expected = Type.Object({
  workflow: Type.String({ pattern: "^W[1-9][0-9]*$" }),
  state_revision: positive,
  authority_epoch: positive,
  plan_revision: Type.Union([positive, Type.Null()]),
}, closed);
const scope = Type.Object({
  project: Type.String({ minLength: 1, maxLength: 128 }),
  paths: Type.Array(Type.String({ minLength: 1, maxLength: 1024 }), { minItems: 1, maxItems: 64 }),
  actions: Type.Array(Type.String({ minLength: 1, maxLength: 1024 }), { minItems: 1, maxItems: 32 }),
}, closed);
const assumptions = Type.Object({
  project: Type.String({ minLength: 1, maxLength: 128 }),
  source_head: Type.String({ pattern: "^([a-f0-9]{40}|[a-f0-9]{64})$" }),
  base_head: Type.String({ pattern: "^([a-f0-9]{40}|[a-f0-9]{64})$" }),
  files: Type.Array(Type.Object({
    path: Type.String({ minLength: 1, maxLength: 1024 }),
    sha256: Type.Union([digest, Type.Null()]),
  }, closed), { minItems: 1, maxItems: 128 }),
}, closed);
const action = (name: string) => ({ action: Type.Literal(name) });
const contextual = (name: string) => Type.Object({ ...action(name), expected }, closed);
const choice = (name: string) => Type.Object({
  ...action(name), expected, question: Type.String(), question_revision: positive,
}, closed);

export const assistantPlanTool: CorralToolSpec = {
  name: "corral_assistant_plan",
  label: "Local design",
  description: "Read or publish this assistant's immutable local design, not a board plan. "
    + "Stage exact UTF-8 chunks of at most 16 KiB at sequential byte offsets (1 MiB total), then "
    + "publish with the complete byte count, SHA-256 and current state/plan/epoch CAS. "
    + "A retry must contain identical bytes. Ready designs require declared scope and checkable "
    + "source assumptions; publication grants no execution consent. Lifecycle actions remain "
    + "explicitly unavailable until their policy services are installed.",
  op: "assistant_plan",
  parameters: Type.Union([
    Type.Object({ ...action("read"), after: Type.Optional(natural), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 8 })) }, closed),
    Type.Object({ ...action("read_revision"), revision: natural, offset: Type.Optional(natural), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 1024 })) }, closed),
    Type.Object({ ...action("stage"), expected, offset: natural, chunk: Type.String({ minLength: 1, maxLength: DESIGN_CHUNK_MAX_BYTES }) }, closed),
    Type.Object({
      ...action("publish"), expected, bytes: Type.Integer({ minimum: 1, maximum: DESIGN_DOCUMENT_MAX_BYTES }),
      digest, stage: Type.Union([Type.Literal("draft"), Type.Literal("ready")]),
      execution_scope: Type.Optional(scope), assumptions: Type.Optional(assumptions),
      scope_changes: Type.Optional(Type.String({ maxLength: 2048 })),
    }, closed),
    Type.Object({ ...action("enter"), input: Type.String() }, closed),
    contextual("checkpoint"), choice("begin_execution"), contextual("pause"), contextual("candidate"), contextual("close"),
  ]),
  payload(params) {
    if (typeof params.chunk === "string" && Buffer.byteLength(params.chunk, "utf8") > DESIGN_CHUNK_MAX_BYTES) {
      throw new Error("Design chunks are at most 16 KiB of UTF-8, not 16K characters");
    }
    return { ...params };
  },
};

export const assistantPlanReviewTool: CorralToolSpec = {
  name: "corral_assistant_plan_review",
  label: "Local plan review",
  description: "Request one asynchronous independent advisory review of an exact immutable local design revision, "
    + "never a board verdict or execution consent. Cite the genuine owner review request. Start returns promptly; "
    + "read preserves reviewed/current revision and source-freshness labels. No automatic re-review or provider fallback.",
  op: "assistant_plan_review",
  parameters: Type.Union([
    Type.Object({ ...action("start"), expected, input: Type.String() }, closed),
    Type.Object({ ...action("read"), review: Type.Optional(Type.String()), offset: Type.Optional(natural) }, closed),
    Type.Object({ ...action("cancel"), expected, review: Type.String() }, closed),
  ]),
  payload: (params) => ({ ...params }),
};

export const assistantHandoffTool: CorralToolSpec = {
  name: "corral_assistant_handoff",
  label: "Handoff to PM",
  description: "Optional explicit transfer of this workflow to the PM, not Ready, dispatch, "
    + "plan acceptance or landing. Currently unavailable until durable handoff is installed.",
  op: "assistant_handoff",
  parameters: Type.Union([
    contextual("preview"), choice("commit"), Type.Object(action("read"), closed),
    contextual("reconcile"), contextual("cancel"), contextual("return_preview"), choice("return"),
  ]),
  payload: (params) => ({ ...params }),
};

export const assistantWorkflowTools: readonly CorralToolSpec[] = [
  assistantPlanTool, assistantPlanReviewTool, assistantHandoffTool,
];
