import { expect, test } from "bun:test";
import { DecisionInbox, inboxIdentity, type InboxSnapshot, type InboxItem } from "./decision-inbox.ts";

const question: InboxItem = { kind: "question", task: "T-1", container: "T-1/R2", key: "T-1/R2/Q1", revision: "3", revision_kind: "question_revision", state: "escalated", pending: true, scope: "demo", reason: "May I edit?", responsible: "owner", detail: ["permission", "blocking"], inspect: "corral question show T-1/R2/Q1 --json", response: "Owner: corral question answer T-1/R2/Q1 --rev 3 --owner --text <answer>", pm_eligible: false };
const snapshot = (items: readonly InboxItem[]): InboxSnapshot => ({ version: 1, workspace: "/isolated", observed_at: "2026-11-02T00:00:00Z", items });

test("inbox refresh preserves exact selection, history and explicit read failures", async () => {
  let value = snapshot([question, { ...question, key: "T-2/R1/Q1", state: "answered", pending: false, response: null }]);
  let fail = false;
  const store = new DecisionInbox(async () => { if (fail) throw new Error("offline"); return value; }, () => {});
  expect(store.status).toContain("Not loaded");
  await store.refresh(); expect(store.visible).toHaveLength(1);
  store.history = true; const historical = store.visible.find(item => item.key === "T-2/R1/Q1");
  if (!historical) throw new Error("missing history");
  store.select(inboxIdentity(historical));
  value = snapshot([...value.items].reverse()); await store.refresh();
  expect(store.current?.key).toBe("T-2/R1/Q1");
  fail = true; await store.refresh();
  expect(store.status).toContain("STALE / unavailable");
  expect(store.visible).toHaveLength(2);
  await expect(store.answerFlow(question, "worker")).rejects.toThrow("offline");
  store.dispose();
});

test("answer-flow inspection rechecks exact revision, closure and role without a mutation callback", async () => {
  let value = snapshot([question]); let reads = 0;
  const store = new DecisionInbox(async () => { reads++; return value; }, () => {});
  await store.refresh();
  await expect(store.answerFlow(question, "worker")).rejects.toThrow("trusted owner");
  await expect(store.answerFlow(question, "pm")).rejects.toThrow("owner-only");
  value = snapshot([{ ...question, revision: "4" }]);
  await expect(store.answerFlow(question, "assistant")).rejects.toThrow("stale_revision");
  value = snapshot([{ ...question, state: "cancelled", pending: false, response: null }]);
  await expect(store.answerFlow(question, "assistant")).rejects.toThrow("closed");
  value = snapshot([question]);
  expect(await store.answerFlow(question, "assistant")).toContain("corral question answer");
  expect(reads).toBe(6);
  store.dispose();
});

test("source namespaces cannot redirect selection or answer-flow validation to a reused question key", async () => {
  const portable = { ...question, kind: "portable_decision", revision: "digest", pm_eligible: false };
  let value = snapshot([question, portable]);
  const store = new DecisionInbox(async () => value, () => {});
  await store.refresh(); store.move(1);
  expect(store.current?.kind).toBe("portable_decision");
  value = snapshot([portable, question]); await store.refresh();
  expect(store.current?.kind).toBe("portable_decision");
  value = snapshot([question]);
  await expect(store.answerFlow(portable, "assistant")).rejects.toThrow("stale_revision");
  expect(store.current).toBeUndefined();
  store.dispose();
});

test("unknown schema is unavailable, never an empty successful inbox", async () => {
  const store = new DecisionInbox(async () => ({ version: 2, items: [] }), () => {});
  await store.refresh(); expect(store.status).toContain("unavailable"); expect(store.snapshot).toBeUndefined();
  store.dispose();
});
