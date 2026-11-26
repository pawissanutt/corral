import { expect, test } from "bun:test";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { ACTIVITY_INPUT_ID, ActivityInputs } from "./activity-inputs.ts";

const journaled = (facts: Record<string, unknown>): SessionEntry => ({
  id: "entry", parentId: null, timestamp: "", type: "message",
  message: { role: "user", timestamp: 0, content: "same input", ...facts },
});

test("recovery accepts only explicit persisted IDs and retains a bounded recent window without evicting live input", () => {
  let turn = 1, ms = 5;
  const inputs = new ActivityInputs(() => turn, () => ms);
  expect(inputs.admit("board-queued")).toBeTrue();
  inputs.restore([journaled({ [ACTIVITY_INPUT_ID]: "board-evicted" }), ...Array.from({ length: 32 }, (_, i) =>
    journaled({ [ACTIVITY_INPUT_ID]: `board-applied-${i}` })), journaled({ corralRequestId: "board-frame-id-only" })]);
  expect(inputs.get("board-queued")).toBe("pending");
  expect(inputs.get("board-evicted")).toBeUndefined();
  expect(inputs.get("board-frame-id-only")).toBeUndefined();
  expect(inputs.get("board-applied-0")).toBe("applied");
  expect(inputs.admit("board-applied-31")).toBeFalse();
  expect(inputs.admit("app-input")).toBeFalse();
  expect(Object.keys(inputs.snapshot())).toHaveLength(33);
  expect(inputs.admit("board-claimed")).toBeTrue();
  inputs.claim("board-claimed");
  ms = 31_000;
  expect(inputs.snapshot()["board-queued"]).toEqual({ state: "pending", quiet_ms: 30_995 });
  turn = 2;
  inputs.settle();
  ms = 31_001;
  expect(inputs.snapshot()["board-claimed"]).toEqual({ state: "failed", turn: 2 });
  expect(inputs.snapshot()["board-queued"]).toEqual({ state: "pending", quiet_ms: 1 });
});
