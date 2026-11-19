import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { FRAME_MAX_BYTES, type SessionInput, type SubmitResult } from "../src/bridge/protocol.ts";
import { type BridgeHandlers, type BridgeServer, startBridge } from "../src/bridge/server.ts";
import { LiveEvents } from "../src/host/live-events.ts";
import type { ChildSnapshot, FleetSnapshot } from "../src/build/types.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";

const dirs: string[] = [];
const servers: BridgeServer[] = [];
afterAll(async () => {
  await Promise.all(servers.map((server) => server.close()));
  for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
});

interface Harness {
  readonly path: string;
  readonly events: EventLog;
  readonly submitted: SessionInput[];
}

interface HarnessOptions {
  readonly capacity?: number;
  readonly submit?: (input: SessionInput) => Promise<SubmitResult>;
  readonly state?: () => Record<string, unknown>;
  readonly inputReplyMs?: number;
  readonly queueState?: BridgeHandlers["queueState"];
  readonly queueWithdraw?: BridgeHandlers["queueWithdraw"];
  readonly liveEvents?: LiveEvents;
}

async function bridge(options: HarnessOptions = {}): Promise<Harness> {
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-bridge-"));
  dirs.push(dir);
  const events = new EventLog(options.capacity);
  const submitted: SessionInput[] = [];
  const server = await startBridge({
    path: join(dir, "host.sock"), sessionId: "fixture", events,
    ...(options.liveEvents ? { liveEvents: options.liveEvents } : {}),
    ...(options.inputReplyMs ? { inputReplyMs: options.inputReplyMs } : {}),
    handlers: testHandlers({
      state: options.state ?? (() => ({ streaming: false })),
      ...(options.queueState ? { queueState: options.queueState } : {}),
      ...(options.queueWithdraw ? { queueWithdraw: options.queueWithdraw } : {}),
      submit: async (input) => {
        submitted.push(input);
        return options.submit ? options.submit(input) : { disposition: "started", turn: submitted.length };
      },
      cancel: async (turn) => ({ cancelled: false, reason: "idle", ...(turn ? { turn } : {}) }),
      shutdown: () => undefined,
    }),
  });
  servers.push(server);
  return { path: server.path, events, submitted };
}

let nextId = 0;
const frame = (command: string, args: Record<string, unknown> = {}) =>
  ({ version: 2, id: `rq-${++nextId}`, command, args });

test("a v2 request needs no secret and the socket and directory are private", async () => {
  const { path } = await bridge();
  expect(statSync(path).mode & 0o777).toBe(0o600);
  expect(statSync(join(path, "..")).mode & 0o777).toBe(0o700);
  const client = await connect(path);
  const reply = await client.request(frame("get_state"));
  expect(reply.ok).toBeTrue();
  expect(reply.version).toBe(2);
  expect(reply.generation).toBeUndefined();
  expect(reply.result).toEqual({ streaming: false });
  client.close();
});

test("retired commands and oversize frames are refused", async () => {
  const { path } = await bridge();
  for (const command of ["questions", "handoff_drain", "owner_turn"]) {
    const client = await connect(path);
    expect((await client.request(frame(command))).error?.code).toBe("unknown_command");
    client.close();
  }
  const big = await connect(path);
  const reply = await big.raw(JSON.stringify(frame("submit_input", { text: "x".repeat(70_000) })));
  expect(reply.error?.code).toBe("frame_too_large");
  await big.closed();
});

test("queue commands validate selectors and need no operation key", async () => {
  const calls: unknown[] = [];
  const { path } = await bridge({
    queueState: limit => { calls.push(limit); return { available: true, items: [], more: false }; },
    queueWithdraw: key => { calls.push(key); return { outcome: "unknown_item" }; },
  });
  const client = await connect(path);
  try {
    for (const limit of [0, 65, 1.5, null, "2"]) expect((await client.request(frame("queue_state", { limit }))).error?.code).toBe("bad_input");
    for (const args of [{}, { key: "x".repeat(129) }, { key: "bad/key" }]) {
      expect((await client.request(frame("queue_withdraw", args))).error?.code).toBe("bad_input");
    }
    expect(calls).toEqual([]);
    expect((await client.request(frame("queue_state"))).result).toMatchObject({ available: true });
    expect((await client.request(frame("queue_withdraw", { key: "k" }))).result).toEqual({ outcome: "unknown_item" });
    expect(calls).toEqual([64, "k"]);
  } finally { client.close(); }
});

test("submit_input accepts frame-sized text with either origin and does not replay by request ID", async () => {
  const { path, submitted } = await bridge();
  const client = await connect(path);
  const args = { text: "x".repeat(16_000), origin: "owner" };
  const request = frame("submit_input", args);
  expect((await client.request(request)).result).toEqual({ request_id: request.id, disposition: "started", turn: 1 });
  expect((await client.request({ ...request, args: { ...args, origin: "auto" } })).result?.turn).toBe(2);
  expect((await client.request(frame("submit_input", { ...args, origin: "test" }))).error?.code).toBe("bad_input");
  expect((await client.request(frame("submit_input", { ...args, busy_policy: "now" }))).error?.code).toBe("bad_input");
  expect(submitted.map(input => [input.origin, input.busyPolicy, input.text.length]))
    .toEqual([["owner", "steer", 16_000], ["auto", "steer", 16_000]]);
  client.close();
});

test("events carry a monotonic sequence, and a cursor behind the ring gets a gap", async () => {
  const { path, events } = await bridge({ capacity: 4 });
  for (let n = 0; n < 6; n++) events.emit("probe", { n });
  const client = await connect(path);
  const page = (await client.request(frame("events_since", { seq: 0 }))).result as {
    events: { seq: number }[]; last: number; gap?: Record<string, unknown>;
  };
  expect(page.events.map((event) => event.seq)).toEqual([3, 4, 5, 6]);
  expect(page.last).toBe(6);
  expect(page.gap).toEqual({ after: 0, resumed: 3, missing: 2, reconcile: true });
  const current = (await client.request(frame("events_since", { seq: 4 }))).result as { events: { seq: number }[]; gap?: unknown };
  expect(current.events.map((event) => event.seq)).toEqual([5, 6]);
  expect(current.gap).toBeUndefined();
  client.close();
});


test("every reply fits a frame: events page by bytes with a cursor, oversize is explicit, the connection stays usable", async () => {
  const { path, events } = await bridge({ capacity: 1024, state: () => ({ blob: "x".repeat(100_000) }) });
  for (let n = 0; n < 300; n++) {
    events.emit("input", {
      request_id: `rq-${n}-${"r".repeat(80)}`.slice(0, 80), idempotency_key: "k".repeat(128), origin: "o".repeat(64),
      busy_policy: "follow_up", disposition: "queued",
    });
  }
  const big = events.emit("probe", { blob: "y".repeat(20_000) });
  const client = await connect(path);
  const seen: number[] = [];
  let cursor = 0;
  let pages = 0;
  for (let more = true; more; pages++) {
    const reply = await client.request(frame("events_since", { seq: cursor }));
    expect(reply.ok).toBeTrue();
    expect(reply.wireBytes).toBeLessThanOrEqual(FRAME_MAX_BYTES);
    const page = reply.result as { events: { seq: number; omitted?: string }[]; next: number; more: boolean };
    seen.push(...page.events.map((event) => event.seq));
    if (page.events.some((event) => event.seq === big.seq)) {
      expect(page.events.find((event) => event.seq === big.seq)?.omitted).toBe("event_too_large");
    }
    cursor = page.next;
    more = page.more;
  }
  expect(pages).toBeGreaterThan(1);
  expect(seen).toEqual(Array.from({ length: 301 }, (_, index) => index + 1));
  const state = await client.request(frame("get_state"));
  expect(state.error?.code).toBe("response_too_large");
  expect(state.wireBytes).toBeLessThanOrEqual(FRAME_MAX_BYTES);
  expect((await client.request(frame("hello"))).result).toEqual({ protocol: 2 });
  client.close();
});

test("a request split inside a multibyte character arrives intact", async () => {
  const { path, submitted } = await bridge();
  const client = await connect(path);
  const text = "A€B \u{1F600}";
  const request = frame("submit_input", { text, busy_policy: "follow_up", origin: "owner" });
  const inside = (value: Record<string, unknown>, extra: number) =>
    Buffer.from(JSON.stringify(value)).indexOf(Buffer.from("€")) + extra;
  const [ready, first] = await client.pipelined(frame("hello"), request, inside(request, 1));
  expect(ready.ok).toBeTrue();
  expect(first.result?.disposition).toBe("started");
  expect(submitted[0]?.text).toBe(text);
  const pair = await client.batch([frame("get_state"), frame("hello")]);
  expect(pair.map((reply) => reply.ok)).toEqual([true, true]);
  expect(submitted).toHaveLength(1);
  client.close();
});

test("live_since resumes the current bounded fleet after child and mail changes, including gaps", async () => {
  const live = new LiveEvents(2);
  const { path } = await bridge({ liveEvents: live });
  const client = await connect(path);
  const empty: FleetSnapshot = { children: [], mail: [], maxParallel: 3 };
  live.observeFleet(empty);
  const idle = (await client.request(frame("live_since", { seq: 0 }))).result;
  expect(idle).toMatchObject({ events: [], last: 0 });
  expect(idle?.fleet).toBeUndefined();
  const child: ChildSnapshot = {
    id: "review", kind: "review", call: "call", title: "Review", brief: "private brief",
    owns: ["private/path"], after: [], profile: "default", model: "provider/model",
    state: "running", createdAt: 1, startedAt: 2, activity: "read private/path",
    edits: [], usage: { input: 3, output: 4, turns: 1, cost: 0.01 },
    steers: [], pulse: [], history: "r", messages: [], sessionFile: "private/transcript",
  };
  live.observeFleet({ ...empty, children: [child] });
  const started = (await client.request(frame("live_since", { seq: 0 }))).result;
  expect(started?.events).toMatchObject([{ seq: 1, kind: "live_fleet" }]);
  expect(started?.fleet).toEqual({
    children: [{ id: "review", kind: "review", title: "Review", state: "running",
      model: "provider/model", started_at: 2, activity: "read", tokens: 7, cost_usd: 0.01 }],
    messages: [], messages_total: 0, max_parallel: 3,
  });
  const mail = Array.from({ length: 101 }, (_, index) => ({
    seq: index + 1, at: index + 1, from: "parent", to: "review", text: "Ready?", delivery: "delivered" as const,
  }));
  live.observeFleet({ ...empty, children: [child], mail });
  const resumed = (await client.request(frame("live_since", { seq: 1 }))).result;
  const fleet = resumed?.fleet as { messages: { seq: number }[]; messages_total: number };
  expect(resumed?.events).toMatchObject([{ seq: 2, kind: "live_fleet" }]);
  expect(fleet.messages.length).toBe(100);
  expect(fleet.messages[0]?.seq).toBe(2);
  expect(fleet.messages.at(-1)?.seq).toBe(101);
  expect(fleet.messages_total).toBe(101);
  expect((await client.request(frame("live_since", { seq: 0 }))).result?.fleet).toEqual(fleet);
  expect((await client.request(frame("live_since", { seq: 2 }))).result?.fleet).toBeUndefined();
  live.events.emit("live_queue", { steer: 0, follow_up: 0 });
  live.events.emit("live_queue", { steer: 0, follow_up: 0 });
  const gap = (await client.request(frame("live_since", { seq: 0 }))).result;
  expect(gap?.gap).toMatchObject({ reconcile: true });
  expect(gap?.fleet).toEqual(fleet);
  live.observeFleet(empty);
  const cleared = (await client.request(frame("live_since", { seq: 4 }))).result;
  expect(cleared?.events).toMatchObject([{ seq: 5, kind: "live_fleet" }]);
  expect(cleared?.fleet).toEqual({ children: [], messages: [], messages_total: 0, max_parallel: 3 });
  live.observeFleet(empty);
  expect(live.events.last).toBe(5);
  expect((await client.request(frame("live_since", { seq: 5 }))).result?.fleet).toBeUndefined();
  client.close();
});
