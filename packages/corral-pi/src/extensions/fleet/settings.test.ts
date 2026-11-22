import { expect, test } from "bun:test";
import type { Role } from "../../roles/types.ts";
import { boundFleetSettings } from "./settings.ts";

const explore = { model: null, max_parallel: 2, max_turns: 20, timeout_secs: 300 };

test("fleet permissions never grant implementation or review to other roles", () => {
  expect(boundFleetSettings("worker", ["fleet", "subagents"], { subagents: explore }).allowedKinds).toEqual(["explore", "slice", "review"]);
  expect(boundFleetSettings("assistant", ["fleet"]).allowedKinds).toEqual(["review"]);
  for (const role of ["reviewer", "build"] satisfies Role[]) {
    expect(boundFleetSettings(role, ["fleet", "subagents"]).allowedKinds).toEqual(["explore"]);
  }
  expect(boundFleetSettings("worker", []).allowedKinds).toEqual([]);
});

test("fleet limits default and validate independently of legacy exploration limits", () => {
  expect(boundFleetSettings("worker", ["fleet"])).toMatchObject({ maxParallel: 32, maxTurns: 60, timeoutSecs: 1800 });
  expect(boundFleetSettings("worker", ["fleet"], { fleet: { max_parallel: 3 } }).maxParallel).toBe(3);
  expect(boundFleetSettings("worker", ["fleet", "subagents"], { fleet: { max_parallel: 32, max_turns: 200, timeout_secs: 7200 }, subagents: explore }))
    .toMatchObject({ maxParallel: 32, maxTurns: 200, timeoutSecs: 7200 });
  for (const fleet of [{ max_parallel: 0 }, { max_parallel: 33 }, { max_turns: 0 }, { max_turns: 201 }, { timeout_secs: 59 }, { timeout_secs: 7201 }, { max_turns: 1.5 }]) {
    expect(() => boundFleetSettings("worker", ["fleet"], { fleet })).toThrow();
  }
});

test("legacy exploration limits apply only when fleet is absent", () => {
  expect(boundFleetSettings("assistant", ["subagents"], { subagents: explore }))
    .toEqual({ allowedKinds: ["explore"], maxParallel: 2, maxTurns: 20, timeoutSecs: 300 });
  expect(() => boundFleetSettings("assistant", ["subagents"], { subagents: { ...explore, max_parallel: 33 } })).toThrow();
});
