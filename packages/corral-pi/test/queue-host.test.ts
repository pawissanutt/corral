import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { QUEUE_ENTRY, QUEUE_OPERATION_ENTRY } from "../src/host/queue.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { codeHost, type CodeFixture } from "./support/code-host.ts";
import { nextEvent } from "./support/wait.ts";

function ledger(fx: CodeFixture, type = QUEUE_ENTRY): Record<string, unknown>[] {
  return readFileSync(fx.host.session.sessionFile!, "utf8").trim().split("\n").map(line => JSON.parse(line))
    .filter(entry => entry.type === "custom" && entry.customType === type).map(entry => entry.data);
}
function snapshot(fx: CodeFixture) {
  return fx.host.queue.snapshot().items as { item: string; origin: string; text?: string }[];
}

// One real host journey: a tool barrier decides both orderings without sleeps or live providers.
test("object withdrawal beats a poll, start beats withdrawal, and evidence precedes both", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    // First-session persistence: no user/assistant message is needed to flush custom evidence.
    expect(fx.host.session.messages.filter(message => message.role === "user" || message.role === "assistant")).toHaveLength(0);
    await fx.host.session.followUp("equal");
    await fx.host.session.followUp("equal");
    const [first, second] = snapshot(fx);
    expect(first!.item).not.toBe(second!.item);
    expect(ledger(fx).map(entry => entry.event)).toEqual(["queued", "queued"]);
    expect(fx.host.queue.withdraw(second!.item).outcome).toBe("withdrawn");
    expect(fx.host.session.getFollowUpMessages()).toEqual(["equal"]);
    fx.host.session.clearQueue();
    expect(ledger(fx).map(entry => entry.event)).toEqual(["queued", "queued", "withdrawn", "withdrawn"]);

    const running = fx.turn([{ toolCalls: [{ callId: "barrier", name: "fixture_hold", arguments: { name: "race" } }] },
      { text: "after tool" }, { text: "after followup" }]);
    await fx.holds.get("race").started;
    const submit = (key: string, mode: "steer" | "follow_up") => fx.host.submit({ text: "equal", busyPolicy: mode,
      idempotencyKey: key, origin: "corral-inbox" }, key);
    expect((await submit("cancel-me", "steer")).disposition).toBe("queued");
    expect((await submit("start-me", "follow_up")).disposition).toBe("queued");
    const withdrawn = fx.host.queue.bridgeWithdraw("cancel-me", "op-cancel");
    expect(withdrawn.outcome).toBe("withdrawn");
    expect(fx.host.queue.bridgeWithdraw("cancel-me", "op-cancel")).toEqual({ ...withdrawn, replayed: true });
    expect(fx.host.queue.bridgeWithdraw("cancel-me", "op-other").first_op_key).toBe("op-cancel");
    expect(() => fx.host.queue.bridgeWithdraw("start-me", "op-cancel")).toThrow("op_conflict");
    const started = nextEvent(fx.events, event => event.kind === "queue_item" && event.event === "started" && event.idempotency_key === "start-me");
    fx.holds.get("race").release("continue");
    const event = await started;
    // The event is after the durable entry, and start has already won the synchronous removal.
    expect(ledger(fx).some(entry => entry.item === event.item && entry.event === "started")).toBeTrue();
    expect(fx.host.queue.bridgeWithdraw("start-me", "op-late")).toMatchObject({ outcome: "already_started", turn: event.turn });
    await running;
    const seen = fx.host.session.messages.filter(message => message.role === "user")
      .map(message => typeof message.content === "string" ? message.content : message.content.filter(part => part.type === "text").map(part => part.text).join(""));
    expect(seen.filter(text => text === "equal")).toHaveLength(1);
    expect(ledger(fx).filter(entry => entry.item === withdrawn.item).map(entry => entry.event)).toEqual(["queued", "withdrawn"]);
    expect(ledger(fx, QUEUE_OPERATION_ENTRY).filter(entry => entry.op_key === "op-cancel")).toHaveLength(1);

    // Both continue drains and native reset/clear use the same hook, not message_start matching.
    for (const mode of ["steer", "followUp"] as const) {
      await fx.host.session[mode]("continue input");
      fixtureTransport().script([{ text: "continued" }]);
      await fx.host.session.agent.continue();
      expect(snapshot(fx)).toEqual([]);
    }
    await fx.host.session.steer("reset");
    await fx.host.session.followUp("reset");
    fx.host.session.agent.reset();
    expect(snapshot(fx)).toEqual([]);
    expect(fx.host.session.getSteeringMessages()).toEqual([]);
    expect(fx.host.session.getFollowUpMessages()).toEqual([]);
  } finally { await fx.close(); }
});

test("pane edit creates a tail identity and fresh assistant owner_input; reload keeps receipts", async () => {
  const fx = await codeHost("assistant", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    await fx.host.session.followUp("same pane text");
    await fx.host.session.followUp("same pane text");
    const [first, second] = snapshot(fx);
    const edited = await fx.host.queue.edit(second!.item, "replacement text");
    expect(edited.outcome).toBe("withdrawn");
    expect(edited.replacement?.disposition).toBe("queued");
    expect(snapshot(fx).map(entry => entry.item)).toEqual([first!.item, edited.replacement!.item]);
    const replacement = ledger(fx).find(entry => entry.item === edited.replacement!.item);
    expect(replacement).toMatchObject({ supersedes: second!.item, text: "replacement text" });
    const ownerInputs = fx.corral.calls().filter(call => call.op === "event" && call.payload.kind === "owner_input");
    expect(ownerInputs.map(call => call.payload.text)).toEqual(["same pane text", "same pane text", "replacement text"]);
    expect(fx.host.session.getFollowUpMessages()[1]).toContain("A-1/g1/i3");
    await fx.host.session.reload();
    expect(fx.host.queue.operation(edited.op_key)).toEqual(edited);
    await fx.host.session.followUp("after reload");
    expect(snapshot(fx).at(-1)!.item).not.toBe(edited.replacement!.item);
    fx.host.session.clearQueue();
  } finally { await fx.close(); }
});

test("journal failure fences consumption and removal; handled/rejected remain distinct from observation", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"],
    extend: pi => pi.registerCommand("fixture-handled", { description: "Fixture handling", async handler() {} }) });
  fx.host.markAttached("interactive");
  try {
    const handled = await fx.host.submit({ text: "/fixture-handled", busyPolicy: "prompt", idempotencyKey: "handled", origin: "corral-inbox" }, "handled");
    expect(handled.disposition).toBe("handled");
    expect(ledger(fx).find(entry => entry.idempotency_key === "handled")?.event).toBe("handled");
    await expect(fx.host.session.followUp("/fixture-handled")).rejects.toThrow("cannot be queued");
    expect(ledger(fx).at(-1)?.event).toBe("rejected");
    await fx.turn([{ text: "seed for continue" }]);
    await fx.host.session.followUp("do not consume");
    const item = snapshot(fx).at(-1)!;
    const manager = fx.host.session.sessionManager;
    const append = manager.appendCustomEntry.bind(manager);
    manager.appendCustomEntry = () => { throw new Error("injected journal failure"); };
    try {
      expect(() => fx.host.queue.withdraw(item.item)).toThrow("queue_persistence_failed");
      expect(fx.host.session.getFollowUpMessages()).toHaveLength(1);
      const requests = fixtureTransport().requests;
      await expect(fx.host.session.agent.continue()).rejects.toThrow("queue_persistence_failed");
      expect(fixtureTransport().requests).toBe(requests);
      expect(fx.host.session.getFollowUpMessages()).toHaveLength(1);
      expect(() => fx.host.session.agent.clearAllQueues()).toThrow("queue_persistence_failed");
      expect(() => fx.host.queue.snapshot()).toThrow("queue_persistence_failed");
    } finally { manager.appendCustomEntry = append; }
  } finally { await fx.close(); }
});
