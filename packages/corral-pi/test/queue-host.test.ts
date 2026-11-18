import { expect, test } from "bun:test";
import { codeHost, type CodeFixture } from "./support/code-host.ts";
import { nextEvent } from "./support/wait.ts";

const queueEvents = (fx: CodeFixture) => fx.since(0).filter(entry => entry.kind === "queue_item");
function snapshot(fx: CodeFixture) {
  return fx.host.queue.snapshot().items as { item: string; origin: string }[];
}

// One real host journey: a tool barrier decides both orderings without sleeps or live providers.
test("object withdrawal beats a poll and start beats withdrawal", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"],
    extend: pi => pi.on("input", event => event.text === "pane restore"
      ? { action: "transform", text: "expanded pane restore" } : { action: "continue" }),
  });
  fx.host.markAttached("interactive");
  try {
    await fx.host.session.followUp("equal");
    await fx.host.session.followUp("equal");
    const [first, second] = snapshot(fx);
    expect(first!.item).not.toBe(second!.item);
    expect(queueEvents(fx).map(entry => entry.event)).toEqual(["queued", "queued"]);
    expect(await fx.host.queue.remove(second!.item)).toBeTrue();
    expect(fx.host.session.getFollowUpMessages()).toEqual(["equal"]);
    fx.host.session.clearQueue();
    await Promise.resolve(); // Queue events publish after the synchronous mutation.
    expect(queueEvents(fx).map(entry => entry.event)).toEqual(["queued", "queued", "withdrawn", "withdrawn"]);

    const running = fx.turn([{ toolCalls: [{ callId: "barrier", name: "fixture_hold", arguments: { name: "race" } }] },
      { text: "after tool" }, { text: "after followup" }]);
    await fx.holds.get("race").started;
    expect((await fx.host.submitLocal({ text: "equal", busyPolicy: "steer", origin: "auto" })).disposition).toBe("queued");
    expect((await fx.host.submitLocal({ text: "equal", busyPolicy: "follow_up", origin: "owner" })).disposition).toBe("queued");
    const cancel = snapshot(fx).find(item => item.origin === "auto")!.item;
    const start = snapshot(fx).find(item => item.origin === "owner")!.item;
    await fx.host.session.steer("pane restore");
    await fx.host.session.followUp("extension kept", undefined, { source: "extension" });
    const external = snapshot(fx).filter(item => item.origin !== "pane");
    const pane = snapshot(fx).find(item => item.origin === "pane");
    expect(fx.host.session.getSteeringMessages()).toContain("expanded pane restore");
    expect(fx.host.session.clearQueue({ restore: true })).toEqual({ steering: ["pane restore"], followUp: [] });
    expect(snapshot(fx)).toEqual(external);
    await Promise.resolve();
    expect(queueEvents(fx).findLast(entry => entry.item === pane?.item)?.event).toBe("withdrawn");
    const withdrawn = fx.host.queue.withdrawInput(cancel);
    expect(withdrawn).toEqual({ outcome: "withdrawn", item: cancel });
    expect(fx.host.queue.withdrawInput(cancel)).toEqual({ outcome: "unknown_item" });
    const started = nextEvent(fx.events, event => event.kind === "queue_item" && event.event === "started" && event.item === start);
    fx.holds.get("race").release("continue");
    const event = await started;
    expect(fx.host.queue.withdrawInput(start)).toEqual({ outcome: "already_started", item: start, turn: event.turn });
    await running;
    const seen = fx.host.session.messages.filter(message => message.role === "user")
      .map(message => typeof message.content === "string" ? message.content : message.content.filter(part => part.type === "text").map(part => part.text).join(""));
    expect(seen.filter(text => text === "equal")).toHaveLength(1);
    expect(queueEvents(fx).filter(entry => entry.item === cancel).map(entry => entry.event)).toEqual(["queued", "withdrawn"]);

    // Both continue drains and native reset/clear use the same hook, not message_start matching.
    for (const mode of ["steer", "followUp"] as const) {
      await fx.host.session[mode]("continue input");
      fx.transport.script([{ text: "continued" }]);
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

test("pulling a pane item for edit and requeuing creates a new tail identity", async () => {
  const fx = await codeHost("assistant", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    await fx.host.session.followUp("same pane text");
    await fx.host.session.followUp("same pane text");
    const [first, second] = snapshot(fx);
    expect(await fx.host.queue.edit(second!.item)).toBe("same pane text");
    await fx.host.session.followUp("replacement text");
    const [kept, replacement] = fx.host.queue.items();
    expect(kept?.key).toBe(first!.item);
    expect(replacement?.key).not.toBe(second!.item);
    expect(replacement?.text).toBe("replacement text");
    expect(fx.host.queue.withdrawInput(second!.item)).toEqual({ outcome: "unknown_item" });
    fx.host.session.clearQueue();
  } finally { await fx.close(); }
});

test("handled and rejected inputs remain distinct from queued or started input", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"],
    extend: pi => pi.registerCommand("fixture-handled", { description: "Fixture handling", async handler() {} }) });
  fx.host.markAttached("interactive");
  try {
    const handled = await fx.host.submitLocal({ text: "/fixture-handled", busyPolicy: "prompt", origin: "owner" });
    expect(handled.disposition).toBe("handled");
    expect(queueEvents(fx).at(-1)).toMatchObject({ event: "handled", origin: "owner" });
    await expect(fx.host.session.followUp("/fixture-handled")).rejects.toThrow("cannot be queued");
    expect(queueEvents(fx).at(-1)?.event).toBe("rejected");
    expect(snapshot(fx)).toEqual([]);
  } finally { await fx.close(); }
});

test("owner queue lists and reorders exact follow-up objects while a turn is running", async () => {
  const fx = await codeHost("worker", { catalog: 10, enabled: ["queue_control"] });
  fx.host.markAttached("interactive");
  try {
    const requests = fx.transport.requests;
    const running = fx.turn([{ toolCalls: [{ callId: "queue-order", name: "fixture_hold", arguments: { name: "queue-order" } }] },
      { text: "after tool" }, { text: "all delivered" }]);
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
    fx.holds.get("queue-order").release("continue");
    await running;
    const delivered = fx.host.session.messages.filter(message => message.role === "user")
      .map(message => typeof message.content === "string" ? message.content
        : message.content.filter(part => part.type === "text").map(part => part.text).join(""))
      .filter(text => text.startsWith("queue "));
    expect(delivered).toEqual(["queue first", "queue third", "queue second"]);
    // The whole backlog drains at the first stop: one reply answers it, in one turn.
    expect(fx.transport.requests - requests).toBe(3);
    const started = queueEvents(fx).filter(entry => entry.event === "started" && entry.mode === "follow_up");
    expect(started.map(entry => entry.item)).toEqual([items[0]!.key, items[2]!.key, items[1]!.key]);
    expect(new Set(started.map(entry => entry.turn)).size).toBe(1);
    expect(fx.host.queue.items()).toEqual([]);
    expect(await fx.host.queue.move(items[2]!.key, -1)).toBeFalse();
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
    expect(queueEvents(fx).findLast(entry => entry.item === second!.key)?.event).toBe("withdrawn");
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
    } finally { await fx.close(); }
  }
});
