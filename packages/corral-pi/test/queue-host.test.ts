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
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"],
    extend: pi => pi.on("input", event => event.text === "pane restore"
      ? { action: "transform", text: "expanded pane restore" } : { action: "continue" }),
  });
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
    await fx.host.session.steer("pane restore");
    await fx.host.session.followUp("extension kept", undefined, { source: "extension" });
    const external = snapshot(fx).filter(item => item.origin !== "pane");
    const pane = snapshot(fx).find(item => item.origin === "pane");
    expect(fx.host.session.getSteeringMessages()).toContain("expanded pane restore");
    expect(fx.host.session.clearQueue({ restore: true })).toEqual({ steering: ["pane restore"], followUp: [] });
    expect(snapshot(fx)).toEqual(external);
    expect(ledger(fx).filter(entry => entry.item === pane?.item).at(-1)).toMatchObject({ event: "withdrawn", actor: "pane_restore" });
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

test("owner queue lists and reorders exact follow-up objects while a turn is running", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    const running = fx.turn([{ toolCalls: [{ callId: "queue-order", name: "fixture_hold", arguments: { name: "queue-order" } }] },
      { text: "after tool" }, { text: "first delivered" }, { text: "third delivered" }, { text: "second delivered" }]);
    await fx.holds.get("queue-order").started;
    for (const text of ["queue first", "queue second", "queue third"]) await fx.host.session.followUp(text);
    const items = fx.host.queue.items();
    expect(items.map(item => [item.text, item.policy, item.editable])).toEqual([
      ["queue first", "follow_up", true], ["queue second", "follow_up", true], ["queue third", "follow_up", true],
    ]);
    const objects = fx.host.session.agent.getQueuedMessages();
    expect(await fx.host.queue.move(items[2]!.key, -1)).toBeTrue();
    expect(fx.host.queue.items().map(item => item.key)).toEqual([items[0]!.key, items[2]!.key, items[1]!.key]);
    const reordered = fx.host.session.agent.getQueuedMessages();
    expect(reordered).toHaveLength(3);
    expect(reordered[0]).toBe(objects[0]);
    expect(reordered[1]).toBe(objects[2]);
    expect(reordered[2]).toBe(objects[1]);
    expect(fx.host.session.getFollowUpMessages()).toEqual(["queue first", "queue third", "queue second"]);
    expect(ledger(fx, QUEUE_OPERATION_ENTRY).at(-1)).toMatchObject({
      reason: "move", target: items[2]!.key, outcome: "moved",
      before: [items[1]!.key, items[2]!.key], after: [items[2]!.key, items[1]!.key],
    });
    fx.holds.get("queue-order").release("continue");
    await running;
    const delivered = fx.host.session.messages.filter(message => message.role === "user")
      .map(message => typeof message.content === "string" ? message.content
        : message.content.filter(part => part.type === "text").map(part => part.text).join(""))
      .filter(text => text.startsWith("queue "));
    expect(delivered).toEqual(["queue first", "queue third", "queue second"]);
    expect(fx.host.queue.items()).toEqual([]);
    expect(await fx.host.queue.move(items[2]!.key, -1)).toBeFalse();
  } finally { await fx.close(); }
});

test("a drain reentering the move journal cannot consume a partially rebuilt queue", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    await fx.turn([{ text: "seed for reentrant continue" }]);
    for (const text of ["race first", "race second", "race third"]) await fx.host.session.followUp(text);
    const items = fx.host.queue.items();
    const manager = fx.host.session.sessionManager;
    const append = manager.appendCustomEntry.bind(manager);
    let drain: Promise<unknown> | undefined;
    manager.appendCustomEntry = (...args) => {
      const result = append(...args);
      const data = args[1];
      if (args[0] === QUEUE_OPERATION_ENTRY && typeof data === "object" && data !== null && "reason" in data && data.reason === "move") {
        drain = fx.host.session.agent.continue().then(() => undefined, error => error);
      }
      return result;
    };
    try {
      expect(await fx.host.queue.move(items[2]!.key, -1)).toBeTrue();
      expect((await drain) instanceof Error).toBeTrue();
      expect(String(await drain)).toContain("queue_busy");
    } finally { manager.appendCustomEntry = append; }
    expect(fx.host.queue.items().map(item => item.key)).toEqual([items[0]!.key, items[2]!.key, items[1]!.key]);
    let duringDrain: Promise<boolean> | undefined;
    manager.appendCustomEntry = (...args) => {
      const result = append(...args);
      const data = args[1];
      if (!duringDrain && args[0] === QUEUE_ENTRY && typeof data === "object" && data !== null
        && "event" in data && data.event === "started") {
        duringDrain = fx.host.queue.move(items[1]!.key, -1);
      }
      return result;
    };
    fixtureTransport().script([{ text: "first" }, { text: "third" }, { text: "second" }]);
    try {
      await fx.host.session.agent.continue();
      expect(await duringDrain).toBeFalse();
    } finally { manager.appendCustomEntry = append; }
    const delivered = fx.host.session.messages.filter(message => message.role === "user")
      .map(message => typeof message.content === "string" ? message.content
        : message.content.filter(part => part.type === "text").map(part => part.text).join(""))
      .filter(text => text.startsWith("race "));
    expect(delivered).toEqual(["race first", "race third", "race second"]);
    expect(fx.host.queue.items()).toEqual([]);
  } finally { await fx.close(); }
});

test("owner pull-to-edit and removal preserve foreign queue entries and their barriers", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    await fx.host.session.followUp("owner first");
    await fx.host.session.followUp("owner second");
    await fx.host.session.followUp("extension notice", undefined, { source: "extension" });
    await fx.host.session.followUp("owner last");
    const [first, second, foreign, last] = fx.host.queue.items();
    expect(foreign).toMatchObject({ text: "extension notice", editable: false });
    expect(await fx.host.queue.remove(foreign!.key)).toBeFalse();
    expect(await fx.host.queue.edit(foreign!.key)).toBeUndefined();
    expect(await fx.host.queue.move(foreign!.key, 1)).toBeFalse();
    expect(await fx.host.queue.move(last!.key, -1)).toBeFalse();
    // Appending cannot implement insertion ahead of an untouched foreign tail.
    expect(await fx.host.queue.move(second!.key, -1)).toBeFalse();
    expect(await fx.host.queue.edit(second!.key)).toBe("owner second");
    expect(fx.host.queue.items().map(item => item.key)).toEqual([first!.key, foreign!.key, last!.key]);
    expect(fx.host.session.getFollowUpMessages()).toEqual(["owner first", "extension notice", "owner last"]);
    expect(ledger(fx).filter(entry => entry.item === second!.key).at(-1)).toMatchObject({
      event: "withdrawn", actor: "pane", reason: "edit",
    });
    expect(await fx.host.queue.remove(first!.key)).toBeTrue();
    expect(await fx.host.queue.remove(first!.key)).toBeFalse();
    expect(fx.host.queue.items().map(item => item.key)).toEqual([foreign!.key, last!.key]);
  } finally { await fx.close(); }
});

test("interactive queue listing remains available behind both queue-control capability gates", async () => {
  for (const capabilities of [{ catalog: 9, enabled: ["queue_control"] }, { catalog: 10, enabled: [] }]) {
    const fx = await codeHost("worker", capabilities);
    fx.host.markAttached("interactive");
    try {
      await fx.host.session.followUp("read-only first");
      await fx.host.session.followUp("read-only second");
      const items = fx.host.queue.items();
      expect(items.map(item => item.text)).toEqual(["read-only first", "read-only second"]);
      expect(await fx.host.queue.move(items[1]!.key, -1)).toBeFalse();
      expect(await fx.host.queue.edit(items[0]!.key)).toBeUndefined();
      expect(await fx.host.queue.remove(items[0]!.key)).toBeFalse();
      expect(fx.host.queue.items()).toEqual(items);
      expect(fx.host.session.getFollowUpMessages()).toEqual(["read-only first", "read-only second"]);
      expect(ledger(fx, QUEUE_OPERATION_ENTRY)).toEqual([]);
    } finally { await fx.close(); }
  }
});
