// Corral tools shared by the worker and reviewer roles: ask, withdraw, evidence and artifact.
// Each tool is one corral operation; payloads carry only what the model chooses, and the
// launcher-bound identity travels in the envelope, never in model parameters.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";

type Params = Record<string, unknown>;

/** Copies the listed keys whose values are present, so absent options never reach the digest. */
export function pick(params: Params, keys: readonly string[]): Params {
  const out: Params = {};
  for (const key of keys) if (params[key] !== undefined) out[key] = params[key];
  return out;
}

const Text = (description: string) => Type.String({ description });
const Strings = (description: string) => Type.Optional(Type.Array(Type.String(), { description }));

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
    options: Strings("Up to six concrete options"),
    recommendation: Type.Optional(Text("The option you recommend and why")),
    context_refs: Strings("Up to sixteen refs (artifact refs, files, task IDs) that support the question"),
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
    + "`corral evidence record --file` takes; read `corral criteria list <task> --json` first).",
  op: "evidence",
  parameters: Type.Object({
    observation: Type.Record(Type.String(), Type.Unknown(), { description: "The EvidenceObservation v1 object" }),
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
