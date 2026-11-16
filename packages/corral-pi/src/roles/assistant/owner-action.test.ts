import { expect, test } from "bun:test";
import type { ChildSnapshot, FleetSnapshot } from "../../build/types.ts";
import { CorralClient, type CorralCall, type CorralReceipt } from "../../tools/corral.ts";
import { defineCorralTool } from "../../tools/define.ts";
import { ownerActionTool } from "./owner-action.ts";

function review(id: string, endedAt: number, verdict: "approve" | "changes", state: ChildSnapshot["state"] = "done"): ChildSnapshot {
  return { id, endedAt, kind: "review", state, call: id, title: id, brief: "Review candidate", owns: [], after: [],
    profile: "reviewer", model: "provider/reviewer", createdAt: 1, startedAt: 2, edits: [],
    usage: { input: 0, output: 0, turns: 1 }, steers: [], pulse: [], history: "d", messages: [],
    verdict: { verdict, summary: id, findings: [], head: "a".repeat(40), at: endedAt } };
}

const params = { kind: "finish_here", target: "cache", input: "A-1/g1/i1",
  args: ["--title", "Fix cache", "--expected-head", "a".repeat(40)] };

function hostTool(fleet: FleetSnapshot) {
  const calls: CorralCall[] = [];
  const client = { call: async (_op: string, call: CorralCall): Promise<CorralReceipt> => {
    calls.push(call);
    return { version: 1, request_id: call.requestId, op: "owner_action", state: "committed", result: {} };
  } } as unknown as CorralClient;
  const tool = defineCorralTool(ownerActionTool, { client, allowedOps: ["owner_action"],
    sessionId: () => "session", currentTurn: () => 1, fleetSnapshot: () => fleet });
  return { tool, calls };
}

test("finish_here attaches the latest settled host review, not an older approval or live verdict", async () => {
  const { tool, calls } = hostTool({ maxParallel: 3,
    children: [review("old", 10, "approve"), review("latest", 20, "changes"), review("live", 30, "approve", "running")],
    mail: [{ seq: 1, at: 20, from: "latest", to: "parent", text: "Needs repair", delivery: "delivered" }] });
  await tool.execute("finish", params, undefined, undefined, undefined as never);
  expect(calls.length).toBe(1);
  expect(calls[0]?.payload.review).toEqual({ reviewer: "latest", model: "provider/reviewer", verdict: "changes",
    summary: "latest", findings: [], head: "a".repeat(40), at: new Date(20).toISOString() });
  expect((calls[0]?.payload.slices as unknown[]).length).toBe(3);
  expect(calls[0]?.payload.messages).toEqual([{ at: new Date(20).toISOString(), from: "latest", to: "parent",
    text: "Needs repair", delivery: "delivered" }]);
});

test("model-supplied review evidence never reaches owner_action", async () => {
  const { tool, calls } = hostTool({ maxParallel: 1, children: [], mail: [] });
  for (const attempted of [
    { ...params, review: { verdict: "approve" } },
    { ...params, args: [...params.args, "--review", '{"verdict":"approve"}'] },
    { ...params, args: [...params.args, "--messages=[]"] },
  ]) {
    const result = await tool.execute("forged", attempted, undefined, undefined, undefined as never);
    expect(result.isError).toBe(true);
  }
  expect(calls.length).toBe(0);
  await tool.execute("clean", params, undefined, undefined, undefined as never);
  expect(calls[0]?.payload.review).toBe(null);
});
