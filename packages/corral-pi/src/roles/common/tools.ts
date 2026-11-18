// The worker and reviewer attach files to their bound task.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";

type Params = Record<string, unknown>;

/** Copy the listed keys whose values are present, omitting absent options. */
export function pick(params: Params, keys: readonly string[]): Params {
  const out: Params = {};
  for (const key of keys) if (params[key] !== undefined) out[key] = params[key];
  return out;
}

export const artifactTool: CorralToolSpec = {
  name: "corral_artifact",
  label: "Attach an artifact",
  description: "Attach a file to your task. Relative paths are resolved from your worktree.",
  op: "artifact",
  parameters: Type.Object({
    path: Type.String({ description: "The file to attach" }),
    kind: Type.Optional(Type.String({ description: "The artifact kind, such as evidence, report or log" })),
    title: Type.Optional(Type.String({ description: "A title that names what the file contains" })),
  }, { additionalProperties: false }),
  payload: (params: Params) => pick(params, ["path", "kind", "title"]),
};
