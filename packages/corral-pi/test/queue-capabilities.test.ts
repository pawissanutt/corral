import { expect, test } from "bun:test";
import type { AgentSession } from "@earendil-works/pi-coding-agent";
import { HostQueue, QUEUE_OPERATIONS_MAX } from "../src/host/queue.ts";

function fixture() {
  const entries: { type: string; data: Record<string, unknown> }[] = [];
  let enabled = true, interactive = true, current = true;
  const queue = new HostQueue({ generation: 3, session: () => session,
    enabled: () => enabled, interactive: () => interactive, current: () => current,
    turn: next => next ? 2 : 1, emit() {}, fence() {},
  });
  const session = { sessionManager: { appendCustomEntry(type: string, data: Record<string, unknown>) { entries.push({ type, data }); } } } as unknown as AgentSession;
  queue.bind("session-full-identity", "a".repeat(64));
  return { queue, entries, enabled: (value: boolean) => { enabled = value; },
    interactive: (value: boolean) => { interactive = value; }, current: (value: boolean) => { current = value; } };
}

test("capability, interaction, binding and transport origin fail closed without touching queue evidence", () => {
  const fx = fixture();
  const tag = fx.queue.observer.input({ text: "owner secret", source: "interactive", mode: "follow_up" });
  const message = { role: "user" as const, content: "owner secret", timestamp: 1 };
  fx.queue.observer.queue({ action: "enqueue", mode: "follow_up", messages: [message], nextRun: false, entries: [{ message, tag }] });
  const id = (fx.queue.snapshot().items as { item: string }[])[0]!.item;
  expect((fx.queue.snapshot(1, false).items as unknown[])[0]).toMatchObject({ item: id, actions: [] });
  expect(JSON.stringify(fx.queue.snapshot(1, false))).not.toContain("owner secret");
  const count = fx.entries.length;
  for (const [change, code] of [[fx.enabled, "capability_unavailable"], [fx.interactive, "not_interactive"], [fx.current, "stale_item"]] as const) {
    change(false);
    expect(() => fx.queue.snapshot()).toThrow(code);
    expect(() => fx.queue.bridgeWithdraw("missing", "op")).toThrow(code);
    expect(() => fx.queue.withdraw(id)).toThrow(code);
    change(true);
  }
  expect(() => fx.queue.bridgeWithdraw(id, "foreign")).toThrow("foreign_origin");
  expect(() => fx.queue.bridgeWithdraw("q-g2-aaaaaaaaaaaa-1", "stale")).toThrow("stale_item");
  // A display prefix is never enough to adopt another full binding.
  expect(() => fx.queue.bind("session-full-identity", "a".repeat(12) + "b".repeat(52))).toThrow("stale_item");
  expect(fx.entries).toHaveLength(count);
  expect((fx.queue.snapshot().items as unknown[])).toHaveLength(1);
  const bridgeTag = fx.queue.bridgeInput({ text: "Corral input", busyPolicy: "steer", idempotencyKey: "inbox-key", origin: "corral-inbox" });
  const bridgeMessage = { role: "user" as const, content: "Corral input", timestamp: 2 };
  fx.queue.observer.queue({ action: "enqueue", mode: "steer", messages: [bridgeMessage], nextRun: false,
    entries: [{ message: bridgeMessage, tag: bridgeTag }] });
  const corralItem = (fx.queue.snapshot().items as { item: string; origin: string }[]).find(item => item.origin === "corral-inbox")!;
  expect(() => fx.queue.withdraw(corralItem.item)).toThrow("use corral inbox withdraw");
  expect(fx.entries).toHaveLength(count + 1);
});

test("operation bound retains canonical replay and changed-intent conflict at capacity", () => {
  const fx = fixture();
  for (let n = 0; n < QUEUE_OPERATIONS_MAX; n++) {
    expect(fx.queue.bridgeWithdraw(`missing-${n}`, `op-${n}`).outcome).toBe("unknown_item");
  }
  expect(fx.queue.bridgeWithdraw("missing-0", "op-0")).toEqual({ outcome: "unknown_item", op_key: "op-0", replayed: true });
  expect(() => fx.queue.bridgeWithdraw("different", "op-0")).toThrow("op_conflict");
  expect(() => fx.queue.bridgeWithdraw("new", "op-new")).toThrow("op_limit");
  expect(fx.entries).toHaveLength(QUEUE_OPERATIONS_MAX);
});
