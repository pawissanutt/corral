// The assistant's owner authority in the real host (T-544 owner-authority, C3 to C5): text the
// owner types into the pane is journaled before the model sees it and tagged with its owner-input
// ID; corral wakes are never journaled; an ordinary owner action is one call with no question;
// a confirm-class action shows the owner the command, journals the answer and sends it once.
import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import type { CapabilitySet, HostIdentity } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { nextEvent } from "./support/wait.ts";

// Answers owner_input with the next journal ID and, like corral, makes a discard wait until an
// answer is journaled for its request ID.
const FAKE = `#!/usr/bin/env bun
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
const dir = dirname(process.argv[1]);
const envelope = JSON.parse(readFileSync(0, "utf8"));
appendFileSync(join(dir, "calls.jsonl"), JSON.stringify({ op: envelope.op, request_id: envelope.request_id,
  payload: envelope.payload }) + "\\n");
const counter = join(dir, "inputs");
const answered = join(dir, "answers");
let state = "committed";
let result = { op: envelope.op };
if (envelope.payload.kind === "owner_input") {
  const n = (existsSync(counter) ? Number(readFileSync(counter, "utf8")) : 0) + 1;
  writeFileSync(counter, String(n));
  if (envelope.payload.answers) appendFileSync(answered, envelope.payload.answers + "\\n");
  result = { kind: "owner_input", input: "A-1/g1/i" + n };
} else if (envelope.op === "owner_action" && envelope.payload.kind === "discard"
  && !(existsSync(answered) && readFileSync(answered, "utf8").split("\\n").includes(envelope.request_id))) {
  state = "rejected";
  result = { code: "confirmation_required", reason: "waits", command: "corral discard " + envelope.payload.target };
} else if (envelope.op === "owner_action") {
  result = { outcome: envelope.payload.kind === "discard" ? "declined" : "executed" };
}
process.stdout.write(JSON.stringify({ version: 1, request_id: envelope.request_id, op: envelope.op, state, result,
  at: new Date().toISOString() }) + "\\n");
`;

interface Call { op: string; request_id: string; payload: Record<string, unknown> }

test("owner text is journaled and tagged, and only the confirm class asks the owner", async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-owner-action-"));
  const cwd = join(work, "ws");
  mkdirSync(join(cwd, ".corral"), { recursive: true });
  const corral = join(work, "corral");
  writeFileSync(corral, FAKE, { mode: 0o700 });
  const calls = (): Call[] => readFileSync(join(work, "calls.jsonl"), "utf8").trim().split("\n")
    .map((line) => JSON.parse(line) as Call);
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity: HostIdentity = { store: "st-demo", project: "", role: "assistant", assistant: "A-1", generation: 1 };
    const events = new EventLog(1);
    const client = new CorralClient({
      corral, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    const capabilities: CapabilitySet = { catalog: 2, digest: "b".repeat(64), preset: "strict", enabled: [] };
    host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, ui: "plain",
      guidance: { projectText: false, settings: {} as never }, client, events, network,
      modules: { ...modules, role: assistantRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    const asked: string[] = [];
    const ui = { confirm: async (_title: string, message: string) => { asked.push(message); return false; } };
    await host.session.bindExtensions({ uiContext: ui as unknown as ExtensionUIContext });
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const live = host;
    const turn = async (run: () => Promise<unknown>) => {
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      await run();
      await ended;
      await live.drained();
    };
    const userText = () => live.session.messages.filter((m) => m.role === "user")
      .map((m) => typeof m.content === "string" ? m.content : m.content.map((p) => p.type === "text" ? p.text : "").join(""));

    // When the owner types an instruction and the model approves T-5 citing it.
    const owner = "T-5 looks good, please land it";
    fixtureTransport().script([
      { toolCalls: [{ callId: "land", name: "corral_owner_action",
        arguments: { kind: "approve", target: "T-5", input: "A-1/g1/i1" } }] },
      { text: "Landed." },
    ]);
    await turn(() => live.session.prompt(owner, { source: "interactive" }));
    // Then the exact text was journaled before the turn, the model saw it tagged, and the
    // approval was one call with no question.
    const first = calls();
    const journal = first.findIndex((call) => call.payload.kind === "owner_input");
    const started = first.findIndex((call) => call.payload.kind === "turn_started");
    expect(journal).toBeGreaterThanOrEqual(0);
    expect(started).toBeGreaterThan(journal);
    expect(first[journal]?.payload).toEqual({ kind: "owner_input", source: "pane", text: owner, sha256: sha256Hex(owner) });
    expect(userText()[0]).toBe(`${owner}\n\n[owner input A-1/g1/i1]`);
    const approve = first.filter((call) => call.op === "owner_action");
    expect(approve.map((call) => call.payload))
      .toEqual([{ kind: "approve", target: "T-5", input: "A-1/g1/i1" }]);
    expect(asked).toEqual([]);

    // When a corral wake arrives, it is neither journaled nor tagged.
    fixtureTransport().script([{ text: "Noted." }]);
    const before = calls().length;
    await turn(() => live.submit({ text: "[corral] T-9 moved to review", busyPolicy: "prompt", idempotencyKey: "w1",
      origin: "corral-inbox" }, "rq-w1"));
    expect(calls().slice(before).some((call) => call.payload.kind === "owner_input")).toBeFalse();
    expect(userText()[1]).toBe("[corral] T-9 moved to review");

    // When the owner asks to drop T-3, the host shows the command, journals the N answer for
    // this request and sends the identical request once more.
    fixtureTransport().script([
      { toolCalls: [{ callId: "drop", name: "corral_owner_action",
        arguments: { kind: "discard", target: "T-3", input: "A-1/g1/i2" } }] },
      { text: "Left as is." },
    ]);
    const mark = calls().length;
    await turn(() => live.session.prompt("drop task T-3", { source: "interactive" }));
    expect(asked).toEqual(["corral discard T-3"]);
    const dropped = calls().slice(mark).filter((call) => call.op === "owner_action" || call.payload.source === "confirmation");
    const request = dropped[0]?.request_id ?? "";
    expect(request).toMatch(/^rq-[0-9a-f]{32}$/);
    expect(dropped.map((call) => call.payload)).toEqual([
      { kind: "discard", target: "T-3", input: "A-1/g1/i2" },
      { kind: "owner_input", source: "confirmation", text: "n", sha256: sha256Hex("n"),
        command: "corral discard T-3", answers: request },
      { kind: "discard", target: "T-3", input: "A-1/g1/i2" },
    ]);
    expect(dropped.filter((call) => call.op === "owner_action").map((call) => call.request_id)).toEqual([request, request]);
  } finally {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
