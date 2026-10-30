// Offline controls only: this test must never request inference.
import { expect, test } from "bun:test";
import { cappedSmokePayload, smokeProfiles } from "../src/host/usage-smoke.ts";

const profiles = ["worker", "reviewer"].flatMap(role => ["strict", "standard"].map(preset => ({
  role, preset, model: "gpt-6.1-sol", prompt: "fixed", tools: [],
})));
const log = (values: unknown[]) => values.map(value => `EARLY_STATIC ${JSON.stringify(value)}`).join("\n");

test("smoke admits exactly four distinct authorized arm controls, ignoring non-cohort static roles", () => {
  const selected = smokeProfiles(log([...profiles, { role: "pm" }, { role: "planner", runnable: false }]));
  expect(selected.map(p => `${p.role}/${p.preset}`)).toEqual([
    "worker/strict", "worker/standard", "reviewer/strict", "reviewer/standard",
  ]);
  expect(() => smokeProfiles(log(profiles.slice(1)))).toThrow("exactly");
  expect(() => smokeProfiles(log([...profiles, profiles[0]]))).toThrow("exactly");
  expect(() => smokeProfiles(log([profiles[0], profiles[0], profiles[2], profiles[3]]))).toThrow("exactly");
  expect(() => smokeProfiles(log(profiles.map(p => ({ ...p, model: "other-model" }))))).toThrow("model control");
});

test("the actual provider payload carries the finite cap even when the adapter omits or raises it", () => {
  expect(cappedSmokePayload({ model: "gpt-6.1-sol", max_output_tokens: 9000 })).toEqual({
    model: "gpt-6.1-sol", max_output_tokens: 1024,
  });
  expect(cappedSmokePayload({ model: "gpt-6.1-sol" }).max_output_tokens).toBe(1024);
  expect(() => cappedSmokePayload(null)).toThrow();
});
