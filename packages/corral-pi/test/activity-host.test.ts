import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startBridge } from "../src/bridge/server.ts";
import { RemoteOwner } from "../src/host/remote-owner.ts";
import { SessionRecord } from "../src/host/session-record.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";
import { codeHost } from "./support/code-host.ts";
import { nextEvent, within } from "./support/wait.ts";

test("a fresh socket sees actual settlement and exact queued feedback pickup across continuation and resend", async () => {
  const end = Promise.withResolvers<void>();
  const next = Promise.withResolvers<void>();
  const preflight = Promise.withResolvers<void>();
  const admit = Promise.withResolvers<void>();
  let boundaries = 0;
  const fx = await codeHost("reviewer", { enabled: [], extend: pi => {
    pi.on("input", async event => {
      if (event.text === "new review") { preflight.resolve(); await admit.promise; }
    });
    pi.on("agent_before_settle", async () => {
      if (++boundaries === 1) { end.resolve(); await next.promise; }
    });
  } });
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-activity-"));
  try {
    const { host, events } = fx;
    const owner = new RemoteOwner(host, events, new SessionRecord(dir, "R1", "reviewer", fx.cwd), host.session.sessionFile!, dir);
    const bridge = await startBridge({ path: join(dir, "host.sock"), sessionId: "R1", events, handlers: testHandlers({
      state: () => host.controlState(owner.paused), submit: (input, id) => owner.submit(input, undefined, id),
      queueWithdraw: key => owner.withdraw(key),
    }) });
    try {
      let seq = 0;
      const call = async (command: string, args: Record<string, unknown> = {}) => {
        const client = await connect(bridge.path);
        try {
          const reply = await client.request({ version: 2, id: `request-${++seq}`, command, args });
          expect(reply.ok).toBeTrue();
          return reply.result;
        } finally { client.close(); }
      };
      const state = async () => (await call("get_state")) as ReturnType<typeof host.controlState>;
      const send = (input_id: string) => call("submit_input", { text: "same text", input_id, origin: "auto", busy_policy: "follow_up" });
      expect((await state()).activity).toEqual({ settled: false, busy: false, waiting: false, turn: 0, inputs: {} });
      const running = fx.turn([
        { text: "initial review" },
        { toolCalls: [{ callId: "hold", name: "fixture_hold", arguments: { name: "feedback" } }] },
        { text: "re-reviewed" },
      ]);
      void running.catch(() => {});
      await within(end.promise, 5_000, "the first pre-settlement boundary");
      // Pi has emitted agent_end, but the SDK is waiting in its pre-settle boundary and may continue.
      expect(await state()).toMatchObject({ turn: { current: 1, open: true }, activity: { settled: false, busy: true } });
      expect(await send("board-feedback")).toMatchObject({ disposition: "queued" });
      expect(await send("board-other")).toMatchObject({ disposition: "queued" });
      expect(await send("board-feedback")).toMatchObject({ disposition: "pending" });
      expect(await call("queue_withdraw", { key: "board-other" })).toMatchObject({ outcome: "withdrawn" });
      expect((await state()).activity.inputs).toMatchObject({
        "board-feedback": { state: "pending" }, "board-other": { state: "failed", turn: 1 },
      });
      next.resolve();
      await within(fx.holds.get("feedback").started, 5_000, "feedback to reach the model");
      expect(await state()).toMatchObject({ turn: { current: 2, open: true }, activity: { settled: false, busy: true,
        inputs: { "board-feedback": { state: "applied", turn: 2 }, "board-other": { state: "failed", turn: 1 } } } });
      fx.holds.get("feedback").release("continue");
      await running;
      expect(await state()).toMatchObject({ turn: { current: 2, open: false }, activity: { settled: true, busy: false } });
      expect(await send("board-feedback")).toMatchObject({ disposition: "handled", reason: "input_already_applied" });
      expect(host.session.messages.filter(m => m.role === "user" && JSON.stringify(m.content).includes("same text"))).toHaveLength(1);

      fx.transport.script([{ text: "new review finished" }]);
      const ended = nextEvent(events, event => event.kind === "turn_ended" && event.turn === 3, 5_000);
      void ended.catch(() => {});
      const requested = call("submit_input", { text: "new review", origin: "auto", input_id: "board-new" });
      void requested.catch(() => {});
      await within(preflight.promise, 5_000, "the next review request's preflight");
      expect(await state()).toMatchObject({ turn: { current: 2, open: false }, activity: { settled: false, busy: true,
        inputs: { "board-new": { state: "pending" } } } });
      admit.resolve();
      expect(await requested).toMatchObject({ disposition: "started", turn: 3 });
      await ended;
      expect((await state()).activity).toMatchObject({ settled: true, inputs: { "board-new": { state: "applied", turn: 3 } } });

      await host.session.followUp("dormant follow-up");
      expect((await state()).activity).toMatchObject({ settled: true, busy: false });
    } finally { await bridge.close(); }
  } finally {
    next.resolve();
    admit.resolve();
    await fx.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("settled host reports explicit todo event waits until the ledger clears them", async () => {
  const fx = await codeHost("assistant", { enabled: ["todo"], catalog: 10 });
  try {
    const { host } = fx;
    const tool = (callId: string, args: Record<string, unknown>) => ({ toolCalls: [{ callId, name: "corral_todo", arguments: args }] });
    await fx.turn([
      tool("init", { op: "init", phases: [{ id: "phase", label: "Phase", steps: [{ id: "step", label: "Step" }] }] }),
      tool("wait", { op: "wait", step: "step", on: { kind: "event", event: "dependency landed" } }),
      { text: "waiting" },
    ]);
    expect(host.controlState(false).activity).toMatchObject({ settled: true, waiting: true });
    await fx.turn([tool("resume", { op: "start", step: "step" }), { text: "ready" }]);
    expect(host.controlState(false).activity).toMatchObject({ settled: true, waiting: false });
  } finally { await fx.close(); }
});
