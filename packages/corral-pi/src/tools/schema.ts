// The schema a model sees for a Corral tool. Provider tool APIs take one root object: pi-ai's
// Anthropic lane replaces a root union (`corral_todo`'s per-op variants) with an empty object, and
// the model then guesses every argument. A root union of objects is advertised as one object; the
// exact union still decides the call before it runs.
import type { TSchema } from "typebox";
import { Errors } from "typebox/value";

export interface JsonSchema {
  type?: string | string[];
  const?: unknown;
  enum?: unknown[];
  anyOf?: JsonSchema[];
  oneOf?: JsonSchema[];
  items?: JsonSchema;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  description?: unknown;
  [key: string]: unknown;
}

function variantsOf(schema: JsonSchema): JsonSchema[] | undefined {
  const variants = schema.anyOf ?? schema.oneOf;
  return schema.properties || !variants || variants.length === 0 ? undefined : variants;
}

/** The key every variant requires with one constant value, such as `op`. */
function discriminator(objects: readonly JsonSchema[]): string | undefined {
  const [first] = objects;
  return Object.keys(first?.properties ?? {}).find((key) => objects.every((variant) =>
    variant.required?.includes(key) && variant.properties?.[key]?.const !== undefined));
}

function plain(schema: JsonSchema): JsonSchema {
  return JSON.parse(JSON.stringify(schema)) as JsonSchema;
}

/**
 * One root object for a root union of objects. Each key accepts any of its variants' schemas and
 * is required only when every variant requires it; constant discriminator values become an enum,
 * and each other key names the discriminator values that use it. Other schemas are unchanged.
 */
export function flatObjectSchema(schema: JsonSchema): JsonSchema {
  const variants = variantsOf(schema);
  if (!variants) return schema;
  const objects = variants.map(flatObjectSchema);
  const tag = discriminator(objects);
  const byKey = new Map<string, Map<string, JsonSchema>>();
  for (const variant of objects) {
    for (const [key, value] of Object.entries(variant.properties ?? {})) {
      const seen = byKey.get(key) ?? new Map<string, JsonSchema>();
      seen.set(JSON.stringify(value), plain(value));
      byKey.set(key, seen);
    }
  }
  const required = [...byKey.keys()].filter((key) => objects.every((variant) => variant.required?.includes(key)));
  const properties: Record<string, JsonSchema> = {};
  for (const [key, seen] of byKey) {
    const unique = [...seen.values()];
    if (key === tag) {
      properties[key] = { type: "string", enum: unique.map((value) => value.const) };
      continue;
    }
    const merged: JsonSchema = unique.length === 1 ? unique[0]! : { anyOf: unique };
    if (tag && !required.includes(key)) {
      const values = (state: "required" | "optional") => objects
        .filter((variant) => variant.properties?.[key] !== undefined
          && (variant.required?.includes(key) ?? false) === (state === "required"))
        .map((variant) => String(variant.properties?.[tag]?.const));
      const notes = ([[values("required"), "Required for"], [values("optional"), "Optional for"]] as const)
        .filter(([ops]) => ops.length > 0)
        .map(([ops, label]) => `${label} ${tag} ${ops.join(", ")}.`);
      const own = typeof merged.description === "string" ? `${merged.description}. ` : "";
      merged.description = own + notes.join(" ");
    }
    properties[key] = merged;
  }
  return { type: "object", properties, required };
}

function structured(schema: JsonSchema | undefined): boolean {
  if (!schema) return false;
  const types = Array.isArray(schema.type) ? schema.type : [schema.type];
  if (types.includes("array") || types.includes("object")) return true;
  return (schema.anyOf ?? schema.oneOf ?? []).some(structured);
}

/**
 * Models sometimes send an array or object argument as its JSON text. Parse a top-level string
 * argument whose advertised schema is structured; anything else passes through for validation.
 */
export function parseStringifiedArguments(advertised: JsonSchema, args: unknown): unknown {
  if (!args || typeof args !== "object" || Array.isArray(args)) return args;
  let prepared: Record<string, unknown> | undefined;
  for (const [key, value] of Object.entries(args as Record<string, unknown>)) {
    if (typeof value !== "string" || !structured(advertised.properties?.[key])) continue;
    try {
      const parsed: unknown = JSON.parse(value);
      if (parsed !== null && typeof parsed === "object") {
        prepared ??= { ...(args as Record<string, unknown>) };
        prepared[key] = parsed;
      }
    } catch {
      // Not JSON: validation reports the string.
    }
  }
  return prepared ?? args;
}

function signature(variant: JsonSchema): string {
  const required = new Set(variant.required ?? []);
  return Object.keys(variant.properties ?? {}).map((key) => required.has(key) ? key : `${key}?`).join(", ");
}

/**
 * Why `params` fails the exact schema, or undefined when it fits. For a discriminated root union
 * the message checks the variant the call names and lists the fields it takes.
 */
export function argumentErrors(schema: TSchema, params: unknown): string | undefined {
  const variants = variantsOf(schema as JsonSchema);
  const tag = variants && discriminator(variants.map(flatObjectSchema));
  const value = params && typeof params === "object" ? (params as Record<string, unknown>)[tag ?? ""] : undefined;
  const variant = tag ? variants?.find((candidate) => candidate.properties?.[tag]?.const === value) : undefined;
  if (tag && variants && !variant) {
    if (Errors(schema, params).length === 0) return undefined;
    return `${tag} must be one of ${variants.map((candidate) => JSON.stringify(candidate.properties?.[tag]?.const)).join(", ")}`;
  }
  const errors = Errors((variant ?? schema) as TSchema, params);
  if (errors.length === 0) return undefined;
  const detail = errors.map((error) => `${error.instancePath || "/"}: ${error.message}`).join("; ");
  return variant && tag ? `${detail}. ${tag} ${String(value)} takes {${signature(variant)}}` : detail;
}
