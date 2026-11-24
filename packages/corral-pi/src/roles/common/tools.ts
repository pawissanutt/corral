import { StringEnum } from "@earendil-works/pi-ai";
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
  description: "Attach a file to a task or project. Relative paths are resolved from the session's working directory.",
  op: "artifact",
  parameters: Type.Object({
    path: Type.String({ description: "The file to attach" }),
    kind: StringEnum(["reference", "report", "evidence"] as const, { description: "The artifact kind" }),
    title: Type.Optional(Type.String({ description: "A title that names what the file contains" })),
    to: Type.Optional(Type.String({ description: "Task ID or project name; defaults to the bound task, then the session's project" })),
    replaces: Type.Optional(Type.String({ description: "Reference returned by the earlier attachment this file revises" })),
  }, { additionalProperties: false }),
  payload: (params: Params) => pick(params, ["path", "kind", "title", "to", "replaces"]),
};
