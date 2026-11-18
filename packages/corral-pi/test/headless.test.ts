// A headless worker starts without a terminal, keeps its question across a transcript reopen,
// and uses the engine CLI to report completion after the owner answers.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { attachHeadless, headlessEnabled } from "../src/host/headless.ts";
import { openSession } from "../src/host/lifecycle.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet, HostIdentity } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { nextEvent } from "./support/wait.ts";

const capabilities: CapabilitySet = {
  catalog: 1, digest: "b".repeat(64), preset: "standard", enabled: ["headless", "todo", "ask_user"],
};

test("a headless worker asks, reopens its transcript and reports done after the owner answers", async () => {
  // Given the production worker role and a run prompt, with no terminal attached:
  expect(headlessEnabled(capabilities)).toBeTrue();
  expect(headlessEnabled({ ...capabilities, enabled: ["todo"] })).toBeFalse();
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-headless-"));
  const originalFetch = globalThis.fetch;
  const hosts: HostSession[] = [];
  try {
    for (const tree of ["src/roles/worker", "src/roles/common", "src/extensions", "assets"]) {
      cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
    }
    const cwd = join(work, "cwd");
    mkdirSync(cwd);
    const promptFile = join(work, "T-1-R1.md");
    writeFileSync(promptFile, "# Task\n\nAsk which adapter to use, then report done.\n");
    restrictEnvironment(process.env);
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const corral = writeFakeEngine(work);
    const sessionDir = join(work, "sessions");
    const stdinListeners = () => process.stdin.listenerCount("data") + process.stdin.listenerCount("readable");
    const listenersBefore = stdinListeners();

    async function start(sessionFile?: string) {
      const identity: HostIdentity = { store: "st-demo", project: "demo", role: "worker", task: "T-1", run: "R1" };
      const events = new EventLog();
      const client = new CorralClient({ corral: corral.path, root: work, cwd, identity });
      const host = await HostSession.create({
        sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, promptFile,
        modules, client, events, network, sessionManager: openSession(sdk, cwd, sessionDir, sessionFile),
      });
      hosts.push(host);
      await host.startLocal();
      const lines: string[] = [];
      return { host, events, lines, attach: () => attachHeadless(host, events, (line) => lines.push(line)) };
    }

    // The headless worker's kickoff turn raises a question before it exits.
    fixtureTransport().script([
      { toolCalls: [{ callId: "ask-1", name: "ask_user", arguments: {
        questions: [{ question: "Which adapter should the reply use?", options: ["socket", "pane"] }],
      } }] },
      { text: "ASKED" },
    ]);
    const first = await start();
    const asked = nextEvent(first.events, (event) => event.kind === "turn_ended" && event.turn === 1);
    const detachFirst = await first.attach();
    await asked;
    await first.host.drained();

    // The question is pending locally, and public state and logging name the headless mode.
    const state = first.host.state();
    expect([state.mode, state.interactive, state.bound]).toEqual(["headless", false, true]);
    const [question] = first.host.pendingOwnerQuestions();
    expect(question).toMatchObject({ question: "Which adapter should the reply use?", status: "pending" });
    if (!question) throw new Error("kickoff did not create a local question");
    expect(first.events.since(0, 64 * 1024).events.find(event => event.kind === "turn_started")?.origin).toEqual({ kind: "unknown" });
    expect(first.lines[0]).toContain("headless: input arrives on the session socket");
    expect(first.lines.some((line) => line.includes("turn 1 ended"))).toBeTrue();
    expect(stdinListeners()).toBe(listenersBefore);
    const sessionId = first.host.sessionId();
    const sessionFile = first.host.session.sessionFile;
    if (!sessionFile) throw new Error("the bound session has no file");
    detachFirst();
    await first.host.close();

    // The resumed host reopens the same transcript, with no new kickoff request.
    const second = await start(sessionFile);
    const requests = fixtureTransport().requests;
    await second.attach();
    expect([second.host.sessionId(), fixtureTransport().requests]).toEqual([sessionId, requests]);
    expect(second.host.pendingOwnerQuestions()).toEqual([question]);
    const summary = "Used the socket adapter.\nVerified: headless fixture\nNot verified: none";
    fixtureTransport().script([
      { toolCalls: [{ callId: "done-1", name: "corral_done", arguments: {
        summary,
      } }] },
    ]);
    const answered = nextEvent(second.events, event => event.kind === "turn_ended");
    await second.host.answerQuestion({ question: question.id, option: "socket" }, async () => undefined);
    const ended = await answered;
    await second.host.drained();
    expect(fixtureTransport().requests - requests).toBe(1); // corral_done terminates the provider turn

    // The local answer starts a bridge-origin turn and completion reaches the engine once.
    const started = second.events.since(0, 64 * 1024).events.filter(event => event.kind === "turn_started");
    expect(started).toHaveLength(1);
    expect(started[0]).toMatchObject({ turn: ended.turn, origin: { kind: "bridge" }, provider: "openai-codex", model: "gpt-6.1-sol" });
    expect(second.host.pendingOwnerQuestions()).toEqual([]);
    expect(corral.calls().filter(argv => argv[3] === "done")).toEqual([
      ["--root", work, "--json", "done", "T-1", "--summary", summary],
    ]);
    expect(second.host.state().mode).toBe("headless");
    expect(stdinListeners()).toBe(listenersBefore);
  } finally {
    for (const host of hosts) await host.close();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
