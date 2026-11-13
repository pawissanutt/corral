import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { InteractiveMode } from "@earendil-works/pi-coding-agent";
import type { Fetch } from "../src/auth/types.ts";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";
import { ScreenModel } from "./support/screen.ts";
import { within } from "./support/wait.ts";

// The owner sees a tool call while its arguments are still being written: a real HostSession, InteractiveMode and
// panel frame, with a provider stream that sends a `write` call's arguments in pieces and holds between them. The
// shared fake Codex transport sends arguments only whole, so this test streams its one call itself (same SSE frames
// as the fake, plus `response.function_call_arguments.delta`) and leaves every other request to the shared fake.

const LINES = Array.from({ length: 12 }, (_, i) => `STREAM_LINE_${String(i + 1).padStart(2, "0")}`);
const CONTENT = LINES.join("\n");

/** The call's argument JSON in three pieces, each ending right after a whole content line. */
function argumentPieces(path: string): readonly string[] {
  const json = JSON.stringify({ path, content: CONTENT });
  const cut = (line: number) => json.indexOf(`${LINES[line - 1]}\\n`) + `${LINES[line - 1]}\\n`.length;
  return [json.slice(0, cut(4)), json.slice(cut(4), cut(9)), json.slice(cut(9))];
}

interface Gate { readonly reached: Promise<void>; reach(): void; readonly released: Promise<void>; release(): void }
function gate(): Gate {
  const reached = Promise.withResolvers<void>(), released = Promise.withResolvers<void>();
  return { reached: reached.promise, reach: reached.resolve, released: released.promise, release: released.resolve };
}

/** One Codex SSE response: a `write` function call whose arguments arrive as `pieces`, waiting at `gates[i]` after piece i. */
function streamedWrite(pieces: readonly string[], gates: readonly Gate[]): Response {
  const encoder = new TextEncoder(), id = "resp_stream_write", itemId = "fc_stream_write", full = pieces.join("");
  const item = { type: "function_call", id: itemId, call_id: "stream-write", name: "write", arguments: full, status: "completed" };
  async function* frames(): AsyncGenerator<Record<string, unknown>> {
    yield { type: "response.created", response: { id, status: "in_progress" } };
    yield { type: "response.output_item.added", output_index: 0, item: { ...item, arguments: "", status: "in_progress" } };
    for (const [index, delta] of pieces.entries()) {
      yield { type: "response.function_call_arguments.delta", output_index: 0, item_id: itemId, delta };
      const hold = gates[index];
      if (hold) { hold.reach(); await hold.released; }
    }
    yield { type: "response.function_call_arguments.done", output_index: 0, item_id: itemId, arguments: full };
    yield { type: "response.output_item.done", output_index: 0, item };
    yield { type: "response.completed", response: { id, status: "completed", usage: {
      input_tokens: 32, input_tokens_details: { cached_tokens: 0 }, output_tokens: 3,
      output_tokens_details: { reasoning_tokens: 0 }, total_tokens: 35 } } };
  }
  const iterator = frames();
  return new Response(new ReadableStream({
    async pull(controller) {
      const next = await iterator.next();
      if (next.done) controller.close();
      else controller.enqueue(encoder.encode(`data: ${JSON.stringify(next.value)}\n\n`));
    },
  }), { status: 200, headers: { "content-type": "text/event-stream" } });
}

test("a write call is visible while its content is still being written, then settles to one row", async () => {
  const work = mkdtempSync(join(tmpdir(), "corral-streaming-rows-")), assets = buildFixtureInstall();
  const oldFetch = globalThis.fetch, oldState = process.env.PI_CODING_AGENT_DIR;
  const gates = [gate(), gate()];
  let host: HostSession | undefined;
  let mode: InteractiveMode | undefined;
  try {
    restrictEnvironment(process.env); process.env.PI_CODING_AGENT_DIR = join(work, "pi-state");
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
    const original = await loadModules(assets.dir, "worker");
    // The fixture worker only reads; this journey needs Pi's own `write` in the loadout.
    const modules = { ...original, role: { ...original.role, upstreamTools: [...original.role.upstreamTools, "write" as const] } };
    const inner = modules.auth.transport?.(authContext());
    if (!inner) throw new Error("fixture transport unavailable");
    const target = join(work, "notes.md");
    let armed = true;
    // The turn's first provider request streams the write; the shared fake answers everything after it.
    const send: Fetch = async (input, init) => {
      if (!armed) return inner(input, init);
      armed = false;
      return streamedWrite(argumentPieces(target), gates);
    };
    const events = new EventLog(1), network = guardedFetch(send);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const corral = writeFakeCorral(work);
    const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules, client, events, network,
      sessionManager: sdk.SessionManager.create(work, join(work, "sessions")) });
    await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
    const panels = host.panels;
    if (!panels) throw new Error("rich host did not install native panels");
    const physical = new RecordingTerminal(), terminal = panels.wrap(physical), screen = new ScreenModel(physical.columns, physical.rows);
    mode = new sdk.InteractiveMode(host.runtime, { terminal, tuiMode: "fullscreen", readOnlyQueue: true, verbose: false, mainView: transcript => panels.mainView(transcript) });
    await mode.init(); host.markAttached("interactive");
    void mode.run().catch(error => { throw error; });
    await physical.untilVisible("WORKER  ● bound");
    physical.send("\x1b[20~"); // F9: Chat, where every session starts; the transcript stays in front

    const lines = () => screen.sync(physical.writes).lines();
    /** The screen row of the write call: its verb and file name on one row. */
    const row = () => lines().find(line => /\bwrite\b/.test(line) && line.includes("notes.md"));
    const visible = (text: string) => lines().some(line => line.includes(text));
    const until = (what: string, holds: () => boolean) => physical.whenWritten(holds, what);

    fixtureTransport().script([{ text: "WRITE_DONE" }]);
    for (const char of "Write the notes.") physical.send(char);
    await physical.until("Write the notes.");
    physical.send("\r");

    // Held after the first piece: the row is already there, says writing, and shows the newest line received.
    await within(gates[0]!.reached, 10_000, "the first argument piece");
    await until("the write row while writing", () => (row() ?? "").includes("writing") && visible(LINES[3]!));
    expect(visible(LINES[4]!)).toBeFalse();

    // Held after the second piece: a later line arrived, and the oldest one scrolled out of the newest-lines body.
    gates[0]!.release();
    await within(gates[1]!.reached, 10_000, "the second argument piece");
    await until("a later streamed line", () => visible(LINES[8]!));
    expect(row()).toContain("writing");
    expect(visible(LINES[0]!)).toBeFalse();
    expect(visible(LINES[9]!)).toBeFalse();

    // The call completes and runs: one row with the line count, no writing, no content rows.
    gates[1]!.release();
    await until("the reply after the write", () => visible("WRITE_DONE"));
    await until("the settled write row", () => (row() ?? "").includes("12 lines"));
    expect(row()).not.toContain("writing");
    expect(lines().some(line => line.includes("writing"))).toBeFalse();
    expect(lines().some(line => line.includes("STREAM_LINE_"))).toBeFalse();
    expect(readFileSync(target, "utf8")).toBe(CONTENT);
    expect(screen.overflow).toBe(0);
  } finally {
    for (const hold of gates) hold.release();
    mode?.stop(); await host?.close(); assets.cleanup(); rmSync(work, { recursive: true, force: true });
    globalThis.fetch = oldFetch;
    if (oldState === undefined) delete process.env.PI_CODING_AGENT_DIR; else process.env.PI_CODING_AGENT_DIR = oldState;
  }
}, 30_000);
