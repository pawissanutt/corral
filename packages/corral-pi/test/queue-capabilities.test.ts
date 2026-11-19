import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AgentSession } from "@earendil-works/pi-coding-agent";
import { HostQueue } from "../src/host/queue.ts";
import { EventLog } from "../src/bridge/events.ts";

test("socket queue metadata previews whole UTF-8 characters and returns full text on withdrawal without pane controls", async () => {
  const events = new EventLog();
  let enabled = false, interactive = true;
  const queued: AgentMessage[] = [];
  const queue = new HostQueue({ session: () => session, enabled: () => enabled, interactive: () => interactive,
    turn: next => next ? 2 : 1, emit: (kind, data) => { events.emit(kind, data); } });
  const session = {
    agent: { getQueuedMessages: () => queued },
    removeQueuedMessage(message: AgentMessage) {
      const index = queued.indexOf(message);
      if (index === -1) return false;
      queued.splice(index, 1);
      queue.observer.queue({ action: "remove", mode: "follow_up", messages: [message], nextRun: false,
        entries: [{ message, tag: undefined }] });
      return true;
    },
  } as unknown as AgentSession;
  const prefix = "owner secret".padEnd(199, "x");
  const remainder = "hidden remainder";
  const text = `${prefix}💬${remainder}`;
  const message = { role: "user" as const, content: text, timestamp: 1 };
  const tag = queue.observer.input({ text: message.content, source: "interactive", mode: "follow_up" });
  queued.push(message);
  queue.observer.queue({ action: "enqueue", mode: "follow_up", messages: [message], nextRun: false, entries: [{ message, tag }] });
  await Promise.resolve();
  const published = events.since(0, 60_000).events;
  expect(published).toHaveLength(1);
  expect(published[0]).toMatchObject({ kind: "queue_item", event: "queued", origin: "pane", mode: "follow_up" });
  expect(JSON.stringify(published)).not.toContain("owner secret");
  expect(queue.observer.restoreInput?.(tag)).toBe(text);
  const [item] = queue.items();
  if (!item) throw new Error("pane item was not indexed");
  expect(item).toMatchObject({ text, editable: true });
  const snapshot = queue.snapshot(1);
  expect(snapshot).toMatchObject({ more: false, items: [{ item: item.key, bytes: Buffer.byteLength(text),
    text_sha256: createHash("sha256").update(text).digest("hex"), actions: ["withdraw"] }] });
  const preview = (snapshot.items as { text: string }[])[0]!.text;
  expect(preview).toBe(prefix);
  expect(Buffer.byteLength(preview)).toBeLessThanOrEqual(200);
  expect(JSON.stringify(snapshot)).not.toContain(remainder);
  expect(await queue.remove(item.key)).toBeFalse();
  enabled = true;
  interactive = false;
  expect(await queue.edit(item.key)).toBeUndefined();
  expect(queue.withdrawInput(item.key, true)).toEqual({ outcome: "withdrawn", item: item.key, text });
  expect(queue.withdrawInput(item.key)).toEqual({ outcome: "unknown_item" });
  expect(queue.items()).toEqual([]);
});
