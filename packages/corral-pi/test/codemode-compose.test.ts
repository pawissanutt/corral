// Code Mode composition semantics through the real HostSession (T-683): owner confirmations suspend
// only their own call; an uncertain nested mutation is reconciled from its receipt and never re-run;
// cancellation keeps committed effects; a detached cell reports exactly once, through the host's
// delivery or the cancel that stopped it; a reload's late completions never reach the replacement
// binding; a replayed child identity returns its one receipt; large intermediates stay in the kernel.
import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { runChild } from "../src/extensions/codemode/child.ts";
import { askTool } from "../src/roles/common/tools.ts";
import { CORRAL_OPS } from "../src/roles/types.ts";
import { CorralClient, toolRequestId } from "../src/tools/corral.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { IDENTITIES, codeHost } from "./support/code-host.ts";
import { writeReceiptCorral } from "./support/receipt-corral.ts";
import { nextEvent } from "./support/wait.ts";

const userTexts = (fx: Awaited<ReturnType<typeof codeHost>>) => fx.host.session.messages
  .filter((message) => message.role === "user")
  .map((message) => typeof message.content === "string" ? message.content
    : message.content.map((part) => part.type === "text" ? part.text : "").join(""));

test("an owner confirmation suspends only its own nested call and needs the owner's actual answer", async () => {
  const fx = await codeHost("assistant");
  try {
    fx.corral.rules({ owner_action: { match: { kind: "discard" }, confirm: true } });
    const code = (gate: string) => `
      const [owner, beside] = await Promise.allSettled([
        tools.corral_owner_action({ kind: "discard", target: "T-3", input: "A-1/g1/i1" }),
        tools.fixture_hold({ name: "${gate}" }),
      ]);
      return { owner: owner.value.state, code: owner.value.result.code ?? null, beside: beside.value };
    `;
    for (const [gate, yes] of [["yes", true], ["no", false]] as const) {
      // When a cell asks for a confirm-class action beside an unrelated permitted call:
      const done = fx.turn([{ toolCalls: [{ callId: `c-${gate}`, name: "codemode", arguments: { code: code(gate) } }] },
        { text: "Finished" }]);
      await fx.confirmations.next();
      // Then the unrelated call runs to completion while the owner's answer is still pending:
      const beside = nextEvent(fx.events, (event) => event.kind === "code_call" && event.phase === "settled"
        && event.tool === "fixture_hold");
      await fx.holds.get(gate).started;
      fx.holds.get(gate).release(`beside-${gate}`);
      await beside;
      expect(fx.confirmations.pending()).toBe(1);
      fx.confirmations.answer(yes);
      await done;
      const text = fx.result(`c-${gate}`).text;
      expect(text).toContain(`"beside": "beside-${gate}"`);
      expect(text).toContain(yes ? `"owner": "committed"` : `"code": "confirmation_required"`);
    }
    // And each attempt was one request ID, resent only after the owner's journaled answer:
    const actions = fx.corral.calls().filter((envelope) => envelope.op === "owner_action");
    const [yesId, noId] = [actions[0]?.request_id, actions[2]?.request_id];
    expect(actions.map((envelope) => envelope.request_id)).toEqual([yesId, yesId, noId, noId]);
    expect(actions.every((envelope) => envelope.identity.parent?.cell !== undefined)).toBe(true);
    const answers = fx.corral.calls().filter((envelope) => envelope.payload.source === "confirmation")
      .map((envelope) => [envelope.payload.text, envelope.payload.answers]);
    expect(answers).toEqual([["y", yesId], ["n", noId]]);
    expect(fx.confirmations.asked).toEqual(["corral owner_action T-3", "corral owner_action T-3"]);
    expect(fx.corral.effects().map((effect) => [effect.op, effect.request_id])).toEqual([["owner_action", yesId]]);
    expect(fx.result("c-no").text)
      .toMatch(/- corral_owner_action \S+\/cell-2\.1 rq-[0-9a-f]{32}: confirmation_required: .*\[effect: rejected\]/);
  } finally {
    await fx.close();
  }
}, 120_000);

test("an uncertain nested mutation is reconciled from its receipt and the cell never re-runs", async () => {
  const fx = await codeHost("worker");
  try {
    fx.corral.rules({ ask: { loseAck: true } });
    const code = `
      const asked = await tools.corral_ask({ kind: "clarification", text: "lost acknowledgement" });
      const found = await tools[asked.error.lookup.tool](asked.error.lookup.arguments);
      return { category: asked.error.category, effect: asked.error.effect, reconciled: found.state, found: found.lookup.found };
    `;
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "codemode", arguments: { code } }] }, { text: "Finished" }]);
    const { text, isError } = fx.result("c1");
    expect(isError).toBe(false);
    expect(text).toContain(`"category": "uncertain_effect",\n  "effect": "unknown",\n  "reconciled": "committed",\n  "found": true`);
    expect(text).toMatch(/- corral_ask \S+ rq-[0-9a-f]{32}: call_failed: .*\[effect: unknown; reconcile with corral_receipt_lookup before any retry\]/);
    // One send, one effect, one cell: nothing retried or re-run automatically.
    expect(fx.corral.calls().filter((envelope) => envelope.op === "ask")).toHaveLength(1);
    expect(fx.corral.effects().map((effect) => effect.op)).toEqual(["ask"]);
    expect(fx.since(0).filter((event) => event.kind === "code_cell" && event.phase === "submitted")).toHaveLength(1);
  } finally {
    await fx.close();
  }
}, 60_000);

test("cancelling a running cell keeps its committed effects and runs nothing after", async () => {
  const fx = await codeHost("worker");
  try {
    const code = `
      await tools.corral_ask({ kind: "clarification", text: "before cancel" });
      await tools.fixture_hold({ name: "stuck" });
      await tools.corral_ask({ kind: "clarification", text: "after cancel" });
    `;
    const settled = nextEvent(fx.events, (event) => event.kind === "code_cell" && event.phase === "settled");
    // An aborted turn makes no further provider request, so only the cell's step is scripted.
    const done = fx.turn([{ toolCalls: [{ callId: "c1", name: "codemode", arguments: { code } }] }]);
    await fx.holds.get("stuck").started;
    expect(await fx.host.cancel(undefined)).toMatchObject({ cancelled: true });
    expect(await settled).toMatchObject({ cell: "cell-1", status: "cancelled", state_lost: true });
    await done;
    const discarded = nextEvent(fx.events, (event) => event.kind === "code_call_discarded" && event.tool === "fixture_hold");
    fx.holds.get("stuck").release("too late");
    await discarded;
    expect(fx.corral.effects().map((effect) => effect.payload.text)).toEqual(["before cancel"]);
    expect(fx.corral.calls().some((envelope) => envelope.payload.text === "after cancel")).toBe(false);
    expect(fx.journal().filter((entry) => entry.tool === "corral_ask").map((entry) => [entry.phase, entry.effect ?? null]))
      .toEqual([["started", null], ["settled", "committed"]]);
  } finally {
    await fx.close();
  }
}, 60_000);

test("a detached cell reports exactly once: one delivered notice, or the cancel that stopped it", async () => {
  const fx = await codeHost("worker");
  try {
    const hold = (gate: string) => `const value = await tools.fixture_hold({ name: "${gate}" }); return value;`;
    // When a cell detaches at once and the model ends its turn:
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "codemode", arguments: { code: hold("slow"), wait_ms: 0 } }] },
      { text: "Waiting for the cell" }]);
    const handle = fx.result("c1");
    expect(handle.isError).toBe(false);
    expect(handle.text).toContain("Cell cell-1: still running after 0 ms");
    expect(handle.text).toContain("do not poll");
    // Then its settlement arrives once, as its own bridge-origin turn:
    fixtureTransport().script([{ text: "Got the result" }]);
    const notified = nextEvent(fx.events, (event) => event.kind === "code_cell_notified");
    const ended = nextEvent(fx.events, (event) => event.kind === "turn_ended");
    fx.holds.get("slow").release("finished-value");
    expect(await notified).toMatchObject({ cell: "cell-1", key: "code:g1:cell-1", disposition: "started" });
    await ended;
    await fx.host.drained();
    const notices = userTexts(fx).filter((text) => text.startsWith("[corral code cell"));
    expect(notices).toHaveLength(1);
    expect(notices[0]).toContain("[corral code cell cell-1 settled: ok]");
    expect(notices[0]).toContain("not an owner instruction");
    expect(notices[0]).toContain("Return value: finished-value");
    const delivered = fx.host.session.sessionManager.getEntries().flatMap((entry) =>
      entry.type === "custom" && entry.customType === "corral_fixture_delivery" ? [entry.data as Record<string, unknown>] : []);
    expect(delivered.filter((entry) => entry.origin === "code-cell").map((entry) => entry.idempotencyKey))
      .toEqual(["code:g1:cell-1"]);
    expect(fx.corral.calls().filter((envelope) => envelope.payload.kind === "turn_started")
      .map((envelope) => envelope.payload.origin).at(-1)).toEqual({ kind: "bridge", idempotency_key: "code:g1:cell-1" });

    // When a second detached cell is cancelled by the model, the cancel result is its report:
    await fx.turn([
      { toolCalls: [{ callId: "c2", name: "codemode", arguments: { code: hold("never"), wait_ms: 0 } }] },
      { toolCalls: [{ callId: "c3", name: "codemode", arguments: { cancel: "cell-2" } }] },
      { text: "Cancelled" },
    ]);
    expect(fx.result("c3").text).toContain("Cell cell-2: cancelled");
    await fx.host.codeNoticed();
    expect(fx.since(0).filter((event) => event.kind === "code_cell_notified").map((event) => event.cell)).toEqual(["cell-1"]);
    expect(fx.result("c3").isError).toBe(false);
    await fx.turn([{ toolCalls: [{ callId: "c4", name: "codemode", arguments: { cancel: "cell-2" } }] }, { text: "Ok" }]);
    expect(fx.result("c4").text).toContain("Cell cell-2: no live cell has this id");
  } finally {
    await fx.close();
  }
}, 60_000);

test("a reload replaces the binding: the old cell settles once, its late completion runs nothing", async () => {
  const fx = await codeHost("worker");
  try {
    const code = `
      let before = 1;
      await tools.fixture_hold({ name: "late" });
      await tools.corral_ask({ kind: "clarification", text: "after reload" });
      return before;
    `;
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "codemode", arguments: { code, wait_ms: 0 } }] }, { text: "Waiting" }]);
    await fx.holds.get("late").started;
    // When the session reloads while the cell's nested call is in flight:
    fixtureTransport().script([{ text: "Noted the lost cell" }]);
    const notified = nextEvent(fx.events, (event) => event.kind === "code_cell_notified");
    const settled = nextEvent(fx.events, (event) => event.kind === "code_cell" && event.phase === "settled");
    await fx.host.session.reload();
    // Then the old cell settles closed with its state lost, reported once after the reload:
    expect(await settled).toMatchObject({ cell: "cell-1", status: "closed", state_lost: true, detached: true });
    expect(await notified).toMatchObject({ cell: "cell-1", disposition: "started" });
    await fx.host.drained();
    const notice = userTexts(fx).find((text) => text.startsWith("[corral code cell cell-1"));
    expect(notice).toContain("settled: closed");
    // And its late completion is discarded: the rest of the old cell never runs.
    const discarded = nextEvent(fx.events, (event) => event.kind === "code_call_discarded" && event.tool === "fixture_hold");
    fx.holds.get("late").release("late result");
    await discarded;
    expect(fx.corral.calls().some((envelope) => envelope.payload.text === "after reload")).toBe(false);
    expect(fx.journal().filter((entry) => entry.tool === "fixture_hold").map((entry) => entry.phase)).toEqual(["started", "settled"]);
    // And the replacement kernel starts empty and serves new cells.
    await fx.turn([{ toolCalls: [{ callId: "c2", name: "codemode", arguments: { code: "return typeof before;" } }] },
      { text: "Finished" }]);
    expect(fx.result("c2").text).toContain("Return value: undefined");
    await fx.host.codeNoticed();
    expect(fx.since(0).filter((event) => event.kind === "code_cell_notified")).toHaveLength(1);
  } finally {
    await fx.close();
  }
}, 60_000);

test("a replayed child identity returns its one receipt, and another payload under it is refused", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-code-replay-"));
  try {
    const corral = writeReceiptCorral(dir);
    const identity = IDENTITIES.worker;
    const client = new CorralClient({ corral: corral.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => "s-1" });
    const tool = defineCorralTool(askTool, { client, allowedOps: CORRAL_OPS, sessionId: () => "s-1", currentTurn: () => 1 });
    const parent = { tool_call_id: "call_code|fc_1", cell: "cell-4", call: "cell-4.2" };
    const child = { key: `${parent.tool_call_id}/${parent.call}`, parent, tool: askTool.name };
    const send = (text: string, piCallId: string) => runChild(child, () =>
      tool.execute(piCallId, { kind: "clarification", text }, undefined, undefined, undefined as never));
    // When the same child is delivered twice (Pi's own nested IDs differ) and then reused for another payload:
    const first = await send("once", `${parent.tool_call_id}/1`);
    const again = await send("once", `${parent.tool_call_id}/9`);
    const other = await send("different", `${parent.tool_call_id}/10`);
    // Then both deliveries name one receipt, the effect applied once, and the reuse is refused:
    const request = toolRequestId("s-1", child.key);
    expect(first.structuredContent).toMatchObject({ request_id: request, state: "committed" });
    expect(again.structuredContent).toMatchObject({ request_id: request, state: "committed", replayed: true });
    expect(other.structuredContent).toMatchObject({ request_id: request, state: "rejected", result: { code: "payload_mismatch" } });
    expect(corral.effects().map((effect) => [effect.request_id, effect.parent])).toEqual([[request, parent]]);
    // And outside a cell the same tool keeps the transcript-derived request ID with no parent.
    const top = await tool.execute("call_top", { kind: "clarification", text: "top" }, undefined, undefined, undefined as never);
    expect(top.structuredContent).toMatchObject({ request_id: toolRequestId("s-1", "call_top") });
    expect(corral.calls().at(-1)?.identity.parent).toBeUndefined();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}, 30_000);

test("a pipeline over a large intermediate result emits only its selected summary", async () => {
  const fx = await codeHost("worker");
  try {
    const code = `const text = await tools.fixture_big({}); return text.split("\\n").find(line => line.startsWith("needle:"));`;
    await fx.turn([
      { toolCalls: [{ callId: "piped", name: "codemode", arguments: { code } }] },
      { toolCalls: [{ callId: "direct", name: "fixture_big", arguments: {} }] },
      { text: "Finished" },
    ]);
    const piped = fx.result("piped").text;
    const direct = fx.host.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.startsWith("direct|"));
    const directText = direct?.role === "toolResult"
      ? direct.content.flatMap((part) => part.type === "text" ? [part.text] : []).join("") : "";
    const measured = { intermediate_bytes: Buffer.byteLength(fx.big) * 2 + 32, piped_emitted_bytes: Buffer.byteLength(piped),
      direct_emitted_bytes: Buffer.byteLength(directText) };
    console.log(`CODEMODE_EMITTED_BYTES ${JSON.stringify(measured)}`);
    expect(piped).toContain("Return value: needle: the selected line");
    expect(measured.piped_emitted_bytes < 256).toBe(true);
    expect(measured.direct_emitted_bytes > measured.intermediate_bytes / 2).toBe(true);
  } finally {
    await fx.close();
  }
}, 60_000);
