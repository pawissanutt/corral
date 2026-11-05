// Corral tools shared by the worker and reviewer roles: ask, withdraw, evidence and artifact.
// Each tool is one corral operation; payloads carry only what the model chooses, and the
// launcher-bound identity travels in the envelope, never in model parameters.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { EvidenceObservation } from "./records.ts";

type Params = Record<string, unknown>;

/** Copies the listed keys whose values are present, so absent options never reach the digest. */
export function pick(params: Params, keys: readonly string[]): Params {
  const out: Params = {};
  for (const key of keys) if (params[key] !== undefined) out[key] = params[key];
  return out;
}

const Text = (description: string) => Type.String({ description });
const Strings = (description: string, maxItems: number) => Type.Optional(Type.Array(Type.String(), { description, maxItems }));

/** Read an existing receipt; this never re-executes the original operation. */
export function receiptLookupTool(pm = false): CorralToolSpec {
  return {
    name: "corral_receipt_lookup", label: "Look up a request receipt", op: "receipt_lookup",
    description: "Reconcile a lost acknowledgement using its request_id. This reads the canonical receipt, "
      + "not a retry of the mutation. Missing is not proof that an in-flight request cannot still commit."
      + (pm ? " For task-local PM comment, reply or question receipts, also supply the task ID." : ""),
    parameters: Type.Object({
      request_id: Type.String({ pattern: "^(rq-[a-f0-9]{32}|ev-[0-9]+-[0-9]+)$" }),
      ...(pm ? { task: Type.Optional(Type.String({ pattern: "^T-[0-9]+$" })) } : {}),
    }, { additionalProperties: false }),
    payload: (params: Params) => pick(params, pm ? ["request_id", "task"] : ["request_id"]),
  };
}

export const askTool: CorralToolSpec = {
  name: "corral_ask",
  label: "Ask corral a question",
  description: "Record a question for the PM (or the owner) and return at once with its key. A blocking question "
    + "still returns immediately: keep working on anything independent, and wait for the answer message before "
    + "relying on it. corral_done refuses while any of your questions is pending; withdraw one you no longer need.",
  op: "ask",
  parameters: Type.Object({
    kind: Type.Union([
      Type.Literal("clarification"), Type.Literal("technical_choice"), Type.Literal("owner_only"),
      Type.Literal("permission"),
    ], { description: "clarification or technical_choice the PM may answer; owner_only and permission always go to the owner" }),
    text: Text("The question, at most 4 KiB"),
    blocking: Type.Optional(Type.Boolean({ description: "Whether your next step depends on the answer" })),
    options: Strings("Up to six concrete options", 6),
    recommendation: Type.Optional(Text("The option you recommend and why")),
    context_refs: Strings("Up to sixteen refs (artifact refs, files, task IDs) that support the question", 16),
    needed_by: Type.Optional(Text("An RFC 3339 time the answer is needed by (a hint only)")),
  }),
  payload: (params: Params) =>
    pick(params, ["kind", "text", "blocking", "options", "recommendation", "context_refs", "needed_by"]),
};

export const withdrawTool: CorralToolSpec = {
  name: "corral_withdraw",
  label: "Withdraw a question",
  description: "Withdraw one of your own pending questions by key, with the reason it is no longer needed.",
  op: "withdraw",
  parameters: Type.Object({ key: Text("The question key corral_ask returned"), reason: Text("Why it is no longer needed") }),
  payload: (params: Params) => pick(params, ["key", "reason"]),
};

export const evidenceTool: CorralToolSpec = {
  name: "corral_evidence",
  label: "Record criterion evidence",
  description: "Record one EvidenceObservation v1 against this task's criteria manifest (the JSON that "
    + "`corral evidence record <task> --file <json>` takes; read `corral criteria list <task>` first, which already returns JSON).",
  op: "evidence",
  parameters: Type.Object({
    observation: EvidenceObservation,
  }),
  payload: (params: Params) => params.observation as Params,
};

export const artifactTool: CorralToolSpec = {
  name: "corral_artifact",
  label: "Attach an artifact",
  description: "Attach a file to this run as an artifact. A relative path is read from your worktree.",
  op: "artifact",
  parameters: Type.Object({
    path: Text("The file to attach"),
    kind: Type.Optional(Type.Union([
      Type.Literal("report"), Type.Literal("plan"), Type.Literal("evidence"), Type.Literal("screenshot"),
      Type.Literal("diff"), Type.Literal("summary"), Type.Literal("log"), Type.Literal("other"),
    ])),
    title: Type.Optional(Text("A title that names what the file proves")),
  }),
  payload: (params: Params) => pick(params, ["path", "kind", "title"]),
};
