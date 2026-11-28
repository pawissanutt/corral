// Code Mode through a real host: cancellation keeps completed tool effects, detached cells
// report once, reload fences late completions, and large intermediates stay in the kernel.
import { expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { codeHost } from "./support/code-host.ts";
import { nextEvent } from "./support/wait.ts";

const userTexts = (fx: Awaited<ReturnType<typeof codeHost>>) => fx.host.session.messages
  .filter((message) => message.role === "user")
  .map((message) => typeof message.content === "string" ? message.content
    : message.content.map((part) => part.type === "text" ? part.text : "").join(""));

test("cancelling a running cell keeps completed writes and runs nothing after", async () => {
  const fx = await codeHost("worker");
  try {
    const code = `
      await tools.write({ path: "before.txt", content: "before cancel" });
      await tools.fixture_hold({ name: "stuck" });
      await tools.write({ path: "after.txt", content: "after cancel" });
    `;
    const settled = nextEvent(fx.events, (event) => event.kind === "code_cell" && event.phase === "settled");
    // An aborted turn makes no further provider request, so only the cell's step is scripted.
    const done = fx.turn([{ toolCalls: [{ callId: "c1", name: "eval_js", arguments: { code } }] }]);
    await fx.holds.get("stuck").started;
    expect(await fx.host.cancel(undefined)).toMatchObject({ cancelled: true });
    expect(await settled).toMatchObject({ cell: "cell-1", status: "cancelled", state_lost: true });
    await done;
    const discarded = nextEvent(fx.events, (event) => event.kind === "code_call_discarded" && event.tool === "fixture_hold");
    fx.holds.get("stuck").release("too late");
    await discarded;
    expect(readFileSync(join(fx.cwd, "before.txt"), "utf8")).toBe("before cancel");
    expect(existsSync(join(fx.cwd, "after.txt"))).toBeFalse();
    expect(fx.journal().filter(entry => entry.tool === "write").map(entry => entry.phase))
      .toEqual(["started", "settled"]);
  } finally {
    await fx.close();
  }
}, 60_000);

test("a detached cell reports exactly once: one delivered notice, or the cancel that stopped it", async () => {
  const fx = await codeHost("worker");
  try {
    const hold = (gate: string) => `const value = await tools.fixture_hold({ name: "${gate}" }); return value;`;
    // When a cell detaches at once and the model ends its turn:
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "eval_js", arguments: { code: hold("slow"), wait_ms: 0 } }] },
      { text: "Waiting for the cell" }]);
    const handle = fx.result("c1");
    expect(handle.isError).toBe(false);
    expect(handle.text).toContain("Cell cell-1: still running after 0 ms");
    expect(handle.text).toContain("do not poll");
    const parent = fx.since(0).find(event => event.kind === "code_cell" && event.phase === "submitted")?.parent;
    if (typeof parent !== "string") throw new Error("cell parent not recorded");
    const key = `code:${parent}:cell-1`;
    // Then its settlement arrives once as its own automated input:
    fx.transport.script([{ text: "Got the result" }]);
    const notified = nextEvent(fx.events, (event) => event.kind === "code_cell_notified");
    const ended = nextEvent(fx.events, (event) => event.kind === "turn_ended");
    fx.holds.get("slow").release("finished-value");
    expect(await notified).toMatchObject({ cell: "cell-1", key, disposition: "started" });
    await ended;
    await fx.host.drained();
    const notices = userTexts(fx).filter(text => text.includes("[corral code cell"));
    expect(notices).toHaveLength(1);
    expect(notices[0]).toContain("[corral code cell cell-1 settled: ok]");
    expect(notices[0]).toContain("not an owner instruction");
    expect(notices[0]).toContain("Return value: finished-value");
    expect(notices[0]).toContain(`[corral notice ${key}]`);

    // When a second detached cell is cancelled by the model, the cancel result is its report:
    await fx.turn([
      { toolCalls: [{ callId: "c2", name: "eval_js", arguments: { code: hold("never"), wait_ms: 0 } }] },
      { toolCalls: [{ callId: "c3", name: "eval_js", arguments: { cancel: "cell-2" } }] },
      { text: "Cancelled" },
    ]);
    expect(fx.result("c3").text).toContain("Cell cell-2: cancelled");
    await fx.host.codeNoticed();
    expect(fx.since(0).filter((event) => event.kind === "code_cell_notified").map((event) => event.cell)).toEqual(["cell-1"]);
    expect(fx.result("c3").isError).toBe(false);
    await fx.turn([{ toolCalls: [{ callId: "c4", name: "eval_js", arguments: { cancel: "cell-2" } }] }, { text: "Ok" }]);
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
      await tools.write({ path: "after-reload.txt", content: "must not run" });
      return before;
    `;
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "eval_js", arguments: { code, wait_ms: 0 } }] }, { text: "Waiting" }]);
    await fx.holds.get("late").started;
    // When the session reloads while the cell's nested call is in flight:
    fx.transport.script([{ text: "Noted the lost cell" }]);
    const notified = nextEvent(fx.events, (event) => event.kind === "code_cell_notified");
    const settled = nextEvent(fx.events, (event) => event.kind === "code_cell" && event.phase === "settled");
    const ended = nextEvent(fx.events, (event) => event.kind === "turn_ended");
    await fx.host.session.reload();
    // Then the old cell settles closed with its state lost, reported once after the reload:
    expect(await settled).toMatchObject({ cell: "cell-1", status: "closed", state_lost: true, detached: true });
    expect(await notified).toMatchObject({ cell: "cell-1", disposition: "started" });
    await ended;
    await fx.host.drained();
    const notice = userTexts(fx).find(text => text.includes("[corral code cell cell-1"));
    expect(notice).toContain("settled: closed");
    // And its late completion is discarded: the rest of the old cell never runs.
    const discarded = nextEvent(fx.events, (event) => event.kind === "code_call_discarded" && event.tool === "fixture_hold");
    fx.holds.get("late").release("late result");
    await discarded;
    expect(existsSync(join(fx.cwd, "after-reload.txt"))).toBeFalse();
    expect(fx.journal().filter((entry) => entry.tool === "fixture_hold").map((entry) => entry.phase)).toEqual(["started", "settled"]);
    // And the replacement kernel starts empty and serves new cells.
    await fx.turn([{ toolCalls: [{ callId: "c2", name: "eval_js", arguments: { code: "return typeof before;" } }] },
      { text: "Finished" }]);
    expect(fx.result("c2").text).toContain("Return value: undefined");
    await fx.host.codeNoticed();
    expect(fx.since(0).filter((event) => event.kind === "code_cell_notified")).toHaveLength(1);
  } finally {
    await fx.close();
  }
}, 60_000);

test("a pipeline over a large intermediate result emits only its selected summary", async () => {
  const fx = await codeHost("worker");
  try {
    const code = `const text = await tools.fixture_big({}); return text.split("\\n").find(line => line.startsWith("needle:"));`;
    await fx.turn([
      { toolCalls: [{ callId: "piped", name: "eval_js", arguments: { code } }] },
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
