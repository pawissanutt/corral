import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { FRAME_MAX_BYTES, type SubmitInput, type SubmitResult } from "../src/bridge/protocol.ts";
import { type BridgeServer, startBridge } from "../src/bridge/server.ts";
import { type PeerLookup, loadPeerLookup } from "../src/host/peer.ts";
import { connect, refusalOnConnect } from "./support/bridge-client.ts";
import { nextEvent } from "./support/wait.ts";

const CAP = "c".repeat(64);
const uid = process.getuid?.() ?? -1;
const dirs: string[] = [];
const servers: BridgeServer[] = [];
afterAll(async () => {
  await Promise.all(servers.map((server) => server.close()));
  for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
});

interface Harness {
  readonly path: string;
  readonly events: EventLog;
  readonly submitted: SubmitInput[];
}

interface HarnessOptions {
  readonly peer?: PeerLookup;
  readonly capacity?: number;
  readonly submit?: (input: SubmitInput) => Promise<SubmitResult>;
  readonly state?: () => Record<string, unknown>;
  readonly submissionsMax?: number;
  readonly inputReplyMs?: number;
}

async function bridge(peerOrOptions?: PeerLookup | HarnessOptions, capacity = 512): Promise<Harness> {
  const options: HarnessOptions = typeof peerOrOptions === "function" ? { peer: peerOrOptions, capacity } : { capacity, ...peerOrOptions };
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-bridge-"));
  dirs.push(dir);
  const events = new EventLog(3, options.capacity ?? 512);
  const submitted: SubmitInput[] = [];
  const server = await startBridge({
    path: join(dir, "host.sock"), ctlCap: CAP, generation: 3, uid, peer: options.peer ?? await loadPeerLookup(), events,
    ...(options.submissionsMax ? { submissionsMax: options.submissionsMax } : {}),
    ...(options.inputReplyMs ? { inputReplyMs: options.inputReplyMs } : {}),
    handlers: {
      hello: () => ({ protocol: 1 }),
      state: options.state ?? (() => ({ streaming: false })),
      submit: async (input) => {
        submitted.push(input);
        return options.submit ? options.submit(input) : { disposition: "started", turn: submitted.length };
      },
      cancel: async (turn) => ({ cancelled: false, reason: "idle", ...(turn ? { turn } : {}) }),
      shutdown: () => undefined,
    },
  });
  servers.push(server);
  return { path: server.path, events, submitted };
}

const frame = (command: string, args: Record<string, unknown> = {}, extra: Record<string, unknown> = {}) =>
  ({ version: 1, id: `rq-${command}-${Math.random().toString(36).slice(2, 10)}`, ctl_cap: CAP, generation: 3, command, args, ...extra });

test("the socket is owner-only and the real peer lookup reads this process's uid", async () => {
  const { path } = await bridge();
  expect(statSync(path).mode & 0o777).toBe(0o600);
  const client = await connect(path);
  const reply = await client.request(frame("get_state"));
  expect(reply.ok).toBeTrue();
  expect(reply.generation).toBe(3);
  expect(reply.result).toEqual({ streaming: false });
  client.close();
});

test("a missing or wrong ctl_cap is refused and the connection closed", async () => {
  const { path } = await bridge();
  for (const auth of [{ ctl_cap: undefined }, { ctl_cap: "x".repeat(64) }, { ctl_cap: CAP.slice(1) }]) {
    const client = await connect(path);
    const reply = await client.request(frame("get_state", {}, auth));
    expect(reply.ok).toBeFalse();
    expect(reply.error?.code).toBe("auth_failed");
    await client.closed();
  }
});

test("a peer with a different uid, or unknown credentials, is refused before any request", async () => {
  for (const peer of [() => ({ pid: 1, uid: uid + 1, gid: 0 }), () => undefined]) {
    const { path } = await bridge(peer);
    const { reply, closed } = await refusalOnConnect(path);
    expect(reply.ok).toBeFalse();
    expect(reply.error?.code).toBe("peer_refused");
    await closed;
  }
});

test("a stale generation, an unknown command and an oversize frame are refused", async () => {
  const { path } = await bridge();
  const client = await connect(path);
  expect((await client.request(frame("get_state", {}, { generation: 2 }))).error?.code).toBe("stale_generation");
  expect((await client.request(frame("exec", {}))).error?.code).toBe("unknown_command");
  const big = await connect(path);
  const reply = await big.raw(JSON.stringify(frame("submit_input", { text: "x".repeat(70_000) })));
  expect(reply.error?.code).toBe("frame_too_large");
  await big.closed();
  client.close();
});

test("submit_input is idempotent by key: a replay returns the first disposition, a changed payload is rejected", async () => {
  const { path, submitted } = await bridge();
  const client = await connect(path);
  const args = { text: "hello", busy_policy: "follow_up", idempotency_key: "msg-1", origin: "inbox" };
  const request = frame("submit_input", args);
  const first = await client.request(request);
  expect(first.result).toEqual({ request_id: request.id, idempotency_key: "msg-1", disposition: "started", turn: 1 });
  const replay = await client.request(frame("submit_input", args));
  expect(replay.result?.disposition).toBe("started");
  expect(replay.result?.replayed).toBeTrue();
  const conflict = await client.request(frame("submit_input", { ...args, text: "changed" }));
  expect(conflict.result?.disposition).toBe("rejected");
  expect(conflict.result?.reason).toBe("idempotency_conflict");
  expect((await client.request(frame("submit_input", { ...args, busy_policy: "now" }))).error?.code).toBe("bad_input");
  expect(submitted).toHaveLength(1);
  client.close();
});

test("events carry the generation and a monotonic sequence, and a cursor behind the ring gets a gap", async () => {
  const { path, events } = await bridge(undefined, 4);
  for (let n = 0; n < 6; n++) events.emit("probe", { n });
  const client = await connect(path);
  const page = (await client.request(frame("events_since", { seq: 0 }))).result as {
    events: { seq: number; generation: number }[]; last: number; gap?: Record<string, unknown>;
  };
  expect(page.events.map((event) => event.seq)).toEqual([3, 4, 5, 6]);
  expect(page.events.every((event) => event.generation === 3)).toBeTrue();
  expect(page.last).toBe(6);
  expect(page.gap).toEqual({ after: 0, resumed: 3, missing: 2, reconcile: true });
  const current = (await client.request(frame("events_since", { seq: 4 }))).result as { events: { seq: number }[]; gap?: unknown };
  expect(current.events.map((event) => event.seq)).toEqual([5, 6]);
  expect(current.gap).toBeUndefined();
  client.close();
});

const input = (key: string, text = "go") => ({ text, busy_policy: "follow_up", idempotency_key: key, origin: "test" });

test("an admitted key is never forgotten: at capacity new keys are refused while replays and conflicts still answer", async () => {
  let release!: (result: SubmitResult) => void;
  const held = new Promise<SubmitResult>((resolve) => { release = resolve; });
  const { path, submitted, events } = await bridge({
    submissionsMax: 3, inputReplyMs: 25,
    submit: (submission) => (submission.text === "hold" ? held : Promise.resolve({ disposition: "handled" })),
  });
  const client = await connect(path);
  expect((await client.request(frame("submit_input", input("k0")))).result?.disposition).toBe("handled");
  expect((await client.request(frame("submit_input", input("k1")))).result?.disposition).toBe("handled");
  expect((await client.request(frame("submit_input", input("k2", "hold")))).result?.disposition).toBe("pending");
  expect((await client.request(frame("submit_input", input("k3")))).error?.code).toBe("input_limit");
  const fresh = await connect(path);
  for (const connection of [client, fresh]) {
    const replay = (await connection.request(frame("submit_input", input("k0")))).result;
    expect([replay?.idempotency_key, replay?.disposition, replay?.replayed]).toEqual(["k0", "handled", true]);
    expect((await connection.request(frame("submit_input", input("k1", "changed")))).result?.reason).toBe("idempotency_conflict");
    const outstanding = (await connection.request(frame("submit_input", input("k2", "hold")))).result;
    expect([outstanding?.disposition, outstanding?.replayed]).toEqual(["pending", true]);
    expect((await connection.request(frame("submit_input", input("k2", "other")))).result?.reason).toBe("idempotency_conflict");
  }
  const settled = nextEvent(events, (event) => event.kind === "input_settled" && event.idempotency_key === "k2");
  release({ disposition: "started", turn: 1 });
  expect([(await settled).disposition, (await settled).turn]).toEqual(["started", 1]);
  const final = (await client.request(frame("submit_input", input("k2", "hold")))).result;
  expect([final?.disposition, final?.turn, final?.replayed]).toEqual(["started", 1, true]);
  expect((await fresh.request(frame("submit_input", input("k4")))).error?.code).toBe("input_limit");
  expect(submitted.map((entry) => entry.idempotencyKey)).toEqual(["k0", "k1", "k2"]);
  client.close();
  fresh.close();
});

test("input upstream has not answered is pending, never rejected, and its outcome follows as input_settled", async () => {
  let release!: (result: SubmitResult) => void;
  const held = new Promise<SubmitResult>((resolve) => { release = resolve; });
  const { path, events } = await bridge({ inputReplyMs: 25, submit: () => held });
  const client = await connect(path);
  const first = (await client.request(frame("submit_input", input("late")))).result;
  expect([first?.disposition, first?.reason]).toEqual(["pending", "preflight_pending"]);
  const settled = nextEvent(events, (event) => event.kind === "input_settled");
  release({ disposition: "rejected", reason: "busy" });
  const outcome = await settled;
  expect([outcome.idempotency_key, outcome.disposition, outcome.reason]).toEqual(["late", "rejected", "busy"]);
  const replay = (await client.request(frame("submit_input", input("late")))).result;
  expect([replay?.disposition, replay?.reason, replay?.replayed]).toEqual(["rejected", "busy", true]);
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
  expect((await client.request(frame("hello"))).result).toEqual({ protocol: 1 });
  client.close();
});

test("a request split inside a multibyte character arrives intact, and a replay chunked differently is the same input", async () => {
  const { path, submitted } = await bridge();
  const client = await connect(path);
  const text = "A€B \u{1F600}";
  const request = frame("submit_input", { text, busy_policy: "follow_up", idempotency_key: "utf8-1", origin: "test" });
  const inside = (value: Record<string, unknown>, extra: number) =>
    Buffer.from(JSON.stringify(value)).indexOf(Buffer.from("€")) + extra;
  const [ready, first] = await client.pipelined(frame("hello"), request, inside(request, 1));
  expect(ready.ok).toBeTrue();
  expect(first.result?.disposition).toBe("started");
  expect(submitted[0]?.text).toBe(text);
  const replayFrame = { ...request, id: "rq-utf8-replay" };
  const [, replay] = await client.pipelined(frame("hello"), replayFrame, inside(replayFrame, 2));
  expect([replay.result?.disposition, replay.result?.replayed]).toEqual(["started", true]);
  const pair = await client.batch([frame("get_state"), frame("hello")]);
  expect(pair.map((reply) => reply.ok)).toEqual([true, true]);
  expect(submitted).toHaveLength(1);
  client.close();
});
