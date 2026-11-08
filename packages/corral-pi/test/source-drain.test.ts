import { expect, test } from "bun:test";
import { attestSourceDrain } from "../src/bridge/source-drain.ts";
import { PlanningPolicy, type PlanningState } from "../src/host/planning-policy.ts";
import { sha256Hex } from "../src/tools/corral.ts";

const pin = { workflow: "W1", state_revision: 4, authority_epoch: 3, plan_revision: null };
const identity = { store: "st-fixture", project: "app", role: "assistant" as const, assistant: "A-1", generation: 2 };
const processIdentity = { pid: 123, start_time: "456" };
const args = { version: 1, transfer: "A-1/W1/source", expected: pin };
const drained: PlanningState = { phase: "handoff_preparing", expected: pin,
  boundary: { active_count: 0, uncertain: false, activities: [], identities_digest: sha256Hex("") } };

test("source proof requires the actual bound drain and exact reserved epoch, never a label or partial activity page", async () => {
  let calls = 0;
  const host = { bound: true, sessionId: () => "native-source", drainPlanning: async () => { calls++; return drained; } };
  const proof = await attestSourceDrain(host, identity, processIdentity, args);
  expect(proof).toEqual({ ...processIdentity, ...args, assistant: "A-1", generation: 2, session_id: "native-source" });
  expect(calls).toBe(1);
  await expect(attestSourceDrain({ ...host, bound: false }, identity, processIdentity, args)).rejects.toThrow();
  expect(calls).toBe(1);
  await expect(attestSourceDrain(host, identity, processIdentity, { ...args, expected: { ...pin, authority_epoch: 2 } })).rejects.toThrow();
  await expect(attestSourceDrain({ ...host, drainPlanning: async () => ({ ...drained,
    boundary: { ...drained.boundary!, active_count: 1 } }) }, identity, processIdentity, args)).rejects.toThrow();
  await expect(attestSourceDrain({ ...host, drainPlanning: async () => { throw new Error("unsupported drain"); } }, identity, processIdentity, args)).rejects.toThrow();
});

test("a drained transfer never re-arms the historical host when work later returns local", async () => {
  let current = { version: 1, ...pin, phase: "handoff_preparing", owner: { kind: "local" }, boundary: drained.boundary };
  const policy = new PlanningPolicy({ read: async () => current, record: async () => { throw new Error("no new authority"); }, fence() {} });
  const host = { bound: true, sessionId: () => "native-source", drainPlanning: () => policy.drained() };
  await attestSourceDrain(host, identity, processIdentity, args);
  current = { ...current, authority_epoch: 5, state_revision: 6, phase: "ordinary" };
  await policy.refresh();
  let writes = 0;
  await expect(policy.execute("write", {}, "late-callback", async () => { writes++; })).rejects.toThrow();
  expect(writes).toBe(0);
});
