// T-579 offline controls/validation, deliberately separate from T-313's six-pair pass logic.
import { createHash } from "node:crypto";
import { z } from "zod";

export const roles = ["worker", "reviewer", "pm", "planner", "plan_reviewer"] as const;
export const Profile = z.object({
  role: z.enum(roles), preset: z.enum(["strict", "standard"]), resource: z.enum(["inert", "configured"]),
  runnable: z.boolean(),
  snapshot: z.object({ catalog: z.number().int().positive(), digest: z.string().regex(/^[a-f0-9]{64}$/),
    features: z.record(z.string(), z.object({ enabled: z.boolean() }).passthrough()) }).passthrough(),
  model: z.literal("gpt-6.1-sol").optional(), inference: z.literal(false).optional(),
  estimator: z.literal("utf8-json-bytes/4-v1").optional(),
  prompt: z.string().optional(), tools: z.array(z.object({ name: z.string(), description: z.string(),
    parameters: z.record(z.string(), z.unknown()) })).optional(),
  prompt_bytes: z.number().int().nonnegative().optional(), tool_bytes: z.number().int().nonnegative().optional(),
  prompt_tokens_estimate: z.number().int().nonnegative().nullable(), tool_tokens_estimate: z.number().int().nonnegative().nullable(),
  prompt_sha256: z.string().optional(), tools_sha256: z.string().optional(),
}).passthrough();
export type Profile = z.infer<typeof Profile>;
export const digest = (s: string) => createHash("sha256").update(s).digest("hex");
export const profileKey = (p: Profile) => `${p.resource}-${p.role}-${p.preset}`;

export function validateProfile(raw: unknown): Profile {
  const p = Profile.parse(raw);
  if (!p.runnable) {
    if (p.preset !== "strict" || !["planner", "plan_reviewer"].includes(p.role) ||
        p.prompt_tokens_estimate !== null || p.tool_tokens_estimate !== null || p.snapshot.features.role_enabled?.enabled !== false) {
      throw new Error("only disabled strict planning trials are not runnable; costs must be null");
    }
    return p;
  }
  if (p.model !== "gpt-6.1-sol" || p.inference !== false || p.estimator !== "utf8-json-bytes/4-v1" ||
      p.prompt === undefined || p.tools === undefined) throw new Error("missing runnable profile controls");
  const toolJson = JSON.stringify(p.tools);
  for (const [text, bytes, tokens, sha] of [
    [p.prompt, p.prompt_bytes, p.prompt_tokens_estimate, p.prompt_sha256],
    [toolJson, p.tool_bytes, p.tool_tokens_estimate, p.tools_sha256],
  ] as const) {
    if (Buffer.byteLength(text) !== bytes || Math.ceil(Buffer.byteLength(text) / 4) !== tokens || digest(text) !== sha) {
      throw new Error("profile bytes, estimate or hash changed");
    }
  }
  return p;
}

/** Preregister 16 requests, no inferred zero-cost PL/PR strict counterpart. */
export function dynamicCohort(raw: unknown[]): Profile[] {
  const profiles = raw.map(validateProfile);
  const keys = profiles.map(profileKey);
  if (new Set(keys).size !== keys.length) throw new Error("duplicate profile");
  const ordered: Profile[] = [];
  for (const resource of ["inert", "configured"] as const) for (const role of roles) {
    for (const preset of ["strict", "standard"] as const) {
      const p = profiles.find(p => p.resource === resource && p.role === role && p.preset === preset);
      if (!p) throw new Error(`missing profile ${resource}/${role}/${preset}`);
      if (p.runnable) ordered.push(p);
    }
  }
  if (profiles.length !== 20 || ordered.length !== 16) throw new Error("expected all 20 role/resource/preset trials, 16 runnable");
  return ordered;
}

/** API equivalents only; subscription billing and SDK error-path zeros are not prices. */
export function apiEquivalent(usage: { input: number; output: number; cacheRead: number; cacheWrite: number }, uncertain: boolean): number | null {
  if (uncertain || [usage.input, usage.output, usage.cacheRead, usage.cacheWrite].some(n => !Number.isFinite(n) || n < 0)) return null;
  return (usage.input * 2 + usage.output * 10 + usage.cacheRead * 0.10 + usage.cacheWrite * 2.50) / 1_000_000;
}
