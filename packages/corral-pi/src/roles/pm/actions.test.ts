import { expect, test } from "bun:test";
import { Check } from "typebox/value";
import { actionTool } from "./actions.ts";

const variants = [
  { action: "act", id: "a".repeat(64) },
  { action: "park", task: "T-007", reason: "Waits for T-006", resume_when: ["status:T-006=done"], resume_action: "Dispatch", review_by: "2h" },
  { action: "attend", key: "T-007/agent_lost", note: "Investigating" },
];

for (const payload of variants) test(`corral_action ${payload.action} sends the pm_action payload`, () => {
  expect(actionTool.op).toBe("pm_action");
  expect(Check(actionTool.parameters, payload)).toBeTrue();
  expect(actionTool.payload(payload)).toEqual(payload);
});

test("corral_action refuses commands, undeclared fields and unknown actions", () => {
  expect(Check(actionTool.parameters, { action: "act", id: "x", command: "corral sync" })).toBeFalse();
  expect(Check(actionTool.parameters, { action: "park", task: "T-007", reason: "r", owner: true })).toBeFalse();
  expect(Check(actionTool.parameters, { action: "land", task: "T-007" })).toBeFalse();
});
