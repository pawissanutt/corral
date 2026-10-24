import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import type { SubmitInput } from "../src/bridge/protocol.ts";
import { type BridgeServer, startBridge } from "../src/bridge/server.ts";
import { type PeerLookup, loadPeerLookup } from "../src/host/peer.ts";
import { connect, refusalOnConnect } from "./support/bridge-client.ts";

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

async function bridge(peer?: PeerLookup, capacity = 512): Promise<Harness> {
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-bridge-"));
  dirs.push(dir);
  const events = new EventLog(3, capacity);
  const submitted: SubmitInput[] = [];
  const server = await startBridge({
    path: join(dir, "host.sock"), ctlCap: CAP, generation: 3, uid, peer: peer ?? await loadPeerLookup(), events,
    handlers: {
      hello: () => ({ protocol: 1 }),
      state: () => ({ streaming: false }),
      submit: async (input) => {
        submitted.push(input);
        return { disposition: "started", turn: submitted.length };
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
