import { expect, test } from "bun:test";
import { apiEquivalent, digest, dynamicCohort, roles, validateProfile } from "../src/capability-profile.ts";

function fixture(role: typeof roles[number], preset: "strict" | "standard", resource: "inert" | "configured") {
  const runnable = preset !== "strict" || !["planner", "plan_reviewer"].includes(role);
  const prompt = "synthetic; not measured", tools: unknown[] = [], toolJson = JSON.stringify(tools);
  return { role, preset, resource, runnable, snapshot: { catalog: 6, digest: "a".repeat(64), features: { role_enabled: { enabled: runnable } } },
    ...(runnable ? { model: "gpt-6.1-sol", inference: false, estimator: "utf8-json-bytes/4-v1", prompt, tools,
      prompt_bytes: Buffer.byteLength(prompt), tool_bytes: Buffer.byteLength(toolJson),
      prompt_sha256: digest(prompt), tools_sha256: digest(toolJson) } : {}),
    prompt_tokens_estimate: runnable ? Math.ceil(Buffer.byteLength(prompt) / 4) : null,
    tool_tokens_estimate: runnable ? Math.ceil(Buffer.byteLength(toolJson) / 4) : null };
}
const fixtures = () => (["inert", "configured"] as const).flatMap(resource => roles.flatMap(role =>
  (["strict", "standard"] as const).map(preset => fixture(role, preset, resource))));

test("all five roles, both resources and presets are mandatory, disabled PL/PR costs stay null", () => {
  const raw = fixtures();
  expect(dynamicCohort(raw)).toHaveLength(16);
  expect(() => dynamicCohort(raw.slice(1))).toThrow("missing profile");
  expect(() => dynamicCohort([...raw, raw[0]])).toThrow("duplicate");
  expect(() => validateProfile({ ...fixture("planner", "strict", "inert"), prompt_tokens_estimate: 0 })).toThrow("costs must be null");
});

test("tampered prompt/schema or heuristic estimate cannot be promoted to measurement", () => {
  const p = fixture("worker", "standard", "configured");
  expect(() => validateProfile({ ...p, prompt: "changed" })).toThrow("hash changed");
  expect(() => validateProfile({ ...p, tools: [{ name: "write", description: "changed", parameters: {} }] })).toThrow("hash changed");
  expect(() => validateProfile({ ...p, prompt_tokens_estimate: 0 })).toThrow("hash changed");
});

test("subscription error zeros remain unknown and cache traffic is priced separately", () => {
  expect(apiEquivalent({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, true)).toBeNull();
  expect(apiEquivalent({ input: 1000, output: 100, cacheRead: 2000, cacheWrite: 50 }, false)).toBeCloseTo(0.003325);
  expect(apiEquivalent({ input: NaN, output: 0, cacheRead: 0, cacheWrite: 0 }, false)).toBeNull();
  const sdk = { input: 1000, output: 100, cacheRead: 2000, cacheWrite: 50, reasoning: 10, totalTokens: 3150,
    cost: { input: 0.002, total: 0.003325 } };
  expect(apiEquivalent(sdk, false)).toBeCloseTo(0.003325);
});
