import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog, type BridgeEvent, type EventPage } from "../src/bridge/events.ts";
import { startBridge } from "../src/bridge/server.ts";
import { LiveEvents } from "../src/host/live-events.ts";
import { RemoteOwner } from "../src/host/remote-owner.ts";
import { installRemote } from "../src/host/remote/index.ts";
import { LIVE_STATE_BYTES, type SessionState } from "../src/host/remote/state.ts";
import { SessionRecord } from "../src/host/session-record.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";
import { codeHost } from "./support/code-host.ts";
import { nextEvent, within } from "./support/wait.ts";

test("live_since carries the state once per change", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-live-state-"));
  const live = new LiveEvents(4);
  const bridge = await startBridge({ path: join(dir, "host.sock"), sessionId: "fixture", events: new EventLog(),
    handlers: testHandlers(), liveEvents: live });
  const client = await connect(bridge.path);
  try {
    const usage = { tokens: 0, cost_partial: false, context_tokens: 0, context_window: 0 };
    const counts = { live: 0, waiting: 0, total: 0 };
    let open = false;
    let observed = Promise.withResolvers<void>();
    live.bindState(() => {
      observed.resolve();
      const state: SessionState = { mode: "headless", turn: open ? 1 : 0, open, paused: false, effort_levels: [], usage,
        queue: Array.from({ length: 12 }, (_, index) => ({ key: `q-${index}`, policy: "steer", origin: "owner",
          text: "\0".repeat(200), bytes: 200, actions: ["withdraw"] })), queue_total: 12,
        cards: [], children: counts, jobs: counts, notices: [], can: [] };
      return { state, parts: {} };
    });
    const since = async (seq: number) => {
      const reply = await client.request({ version: 2, command: "live_since", id: "live", args: { seq } });
      expect(reply.ok).toBeTrue();
      return reply.result as unknown as EventPage & { state?: SessionState };
    };
    const first = await since(0);
    expect(first.events.map(event => event.kind)).toEqual(["live_state"]);
    expect(first.state).toMatchObject({ open: false, queue_total: 12 });
    expect(Buffer.byteLength(JSON.stringify(first.state))).toBeLessThanOrEqual(LIVE_STATE_BYTES);
    expect((await since(first.next)).state).toBeUndefined();

    observed = Promise.withResolvers<void>();
    live.stateChanged();
    await within(observed.promise, 2000, "unchanged state observation");
    expect(await since(first.next)).toMatchObject({ next: first.next, events: [] });
    open = true;
    const changed = nextEvent(live.events, event => event.kind === "live_state", 2000);
    live.stateChanged();
    live.stateChanged();
    await changed;
    const second = await since(first.next);
    expect(second.events.map(event => event.kind)).toEqual(["live_state"]);
    expect(second.state).toMatchObject({ open: true, turn: 1 });
    expect((await since(second.next)).state).toBeUndefined();

    for (let index = 0; index < 5; index++) live.events.emit("probe");
    const gap = await since(second.next);
    expect(gap.gap?.reconcile).toBeTrue();
    expect(gap.state).toEqual(second.state);
  } finally {
    client.close();
    live.close();
    await bridge.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a queued steer is announced under its input id and again when it starts", async () => {
  const fx = await codeHost("assistant", { enabled: [] });
  const dir = mkdtempSync(join(tmpdir(), "corral-live-input-"));
  const { host, events } = fx;
  host.markAttached("headless");
  const file = host.session.sessionFile!;
  const record = new SessionRecord(dir, "A-1", "assistant", fx.cwd);
  record.save(file);
  const owner = new RemoteOwner(host, events, record, file, dir);
  installRemote(host, owner, dir);
  const bridge = await startBridge({ path: join(dir, "host.sock"), sessionId: "A-1", events, liveEvents: host.liveEvents,
    handlers: testHandlers({ submit: (input, requestId) => owner.submit(input, undefined, requestId),
      queueState: limit => owner.queueState(limit) }) });
  const client = await connect(bridge.path);
  const turn = fx.turn([{ toolCalls: [{ callId: "hold", name: "fixture_hold", arguments: { name: "queue" } }] }, { text: "settled" }]);
  try {
    await fx.holds.get("queue").started;
    const submit = await client.request({ version: 2, id: "frame-1", command: "submit_input", args: {
      input_id: "app-steer-1", text: "Owner steer", origin: "owner", busy_policy: "steer", to: "parent",
      typed_at: "2026-10-30T02:03:04Z", device: "phone",
    } });
    expect(submit.result).toEqual({ input: "app-steer-1", request_id: "frame-1", disposition: "queued" });
    const queue = await client.request({ version: 2, id: "queue", command: "queue_state", args: {} });
    expect(queue.result?.items).toMatchObject([{ item: "app-steer-1", mode: "steer", position: 1, text: "Owner steer", to: "parent" }]);
    const started = nextEvent(host.liveEvents.events, event => event.kind === "live_input" && event.state === "started" && event.input === "app-steer-1");
    fx.holds.get("queue").release("continue");
    await started;
    await turn;
    const page = await client.request({ version: 2, id: "live", command: "live_since", args: { seq: 0 } });
    const lifecycle = (page.result?.events as BridgeEvent[]).filter(event => event.kind === "live_input" && event.input === "app-steer-1");
    expect(lifecycle).toMatchObject([
      { state: "accepted" }, { state: "queued", policy: "steer", position: 1 }, { state: "started", turn: 1 },
    ]);
    const entries = readFileSync(file, "utf8").trimEnd().split("\n").map(line => JSON.parse(line));
    const matches = entries.filter(entry => entry.message?.corralRequestId === "app-steer-1");
    expect(matches).toHaveLength(1);
    expect(matches[0]?.message).toMatchObject({ role: "user", corralInput: {
      origin: "owner", to: "parent", typed_at: "2026-10-30T02:03:04Z", device: "phone",
    } });
  } finally {
    client.close();
    await bridge.close();
    await fx.close();
    await turn.catch(() => undefined);
    rmSync(dir, { recursive: true, force: true });
  }
});
