import { expect, test } from "bun:test";

import { sessionTodoTool } from "./todo.ts";

test("session wait forwards typed conditions unchanged alongside its deadline", () => {
  const when = ["status:T-001=done", "answered:T-002/R1/Q1@2"];
  const params = {
    op: "wait", step: "gate", plan: "P1",
    on: { kind: "event", event: "task finishes" },
    deadline: "2026-11-11T14:00:00Z", when,
  };
  const payload = sessionTodoTool.payload(params);
  expect(payload).toEqual(params);
  expect(payload.when).toBe(when);
});
