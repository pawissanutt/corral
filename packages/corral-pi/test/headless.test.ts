// Headless host mode (T-577): the production worker attached without InteractiveMode asks through
// its kickoff turn, a resumed generation reattaches to the same session without a second kickoff,
// and the answer arrives only through the bridge's submit path, where the worker reports done.
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
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const capabilities: CapabilitySet = {
  catalog: 1, digest: "b".repeat(64), preset: "standard", enabled: ["headless", "todo"],
};

test("a headless worker asks, resumes in the same session and completes through the bridge alone", async () => {
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
    const promptFile = join(work, "T-577-R1.md");
    writeFileSync(promptFile, "# Task\n\nAsk which adapter to use, then report done.\n");
    restrictEnvironment(process.env);
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const corral = writeFakeCorral(work);
    const sessionDir = join(work, "sessions");
    const stdinListeners = () => process.stdin.listenerCount("data") + process.stdin.listenerCount("readable");
    const listenersBefore = stdinListeners();

    async function start(generation: number, sessionFile?: string) {
      const identity: HostIdentity = {
        store: "st-demo", project: "demo", role: "worker", task: "T-577", run: "R1", generation,
      };
      const events = new EventLog(generation);
      let host: HostSession | undefined;
      const client = new CorralClient({
        corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
      });
      host = await HostSession.create({
        sdk, identity, cwd, agentDir: join(work, `agent-${generation}`), model: "gpt-6.1-sol", capabilities, promptFile,
        modules, client, events, network, sessionManager: openSession(sdk, cwd, sessionDir, generation, sessionFile),
      });
      hosts.push(host);
      await host.hello(sha256Hex(`nonce-${generation}`), "pane-1", sha256Hex("c".repeat(64)));
      const lines: string[] = [];
      return { host, events, lines, attach: () => attachHeadless(host, events, (line) => lines.push(line)) };
    }
    const ops = () => corral.calls().map(({ envelope }) => envelope);

    // When generation 1 attaches headless and its kickoff turn asks a blocking question:
    fixtureTransport().script([
      { toolCalls: [{ callId: "ask-1", name: "corral_ask", arguments: {
        kind: "clarification", text: "Which adapter should the reply use?", blocking: true, options: ["inbox", "pane"],
      } }] },
      { text: "ASKED" },
    ]);
    const first = await start(1);
    const asked = nextEvent(first.events, (event) => event.kind === "turn_ended" && event.turn === 1);
    await first.attach();
    await asked;
    await first.host.drained();

    // Then the kickoff ran without a terminal, the question reached corral as one receipted ask,
    // and state and the pane log name the headless mode.
    const state = first.host.state();
    expect([state.mode, state.interactive, state.bound]).toEqual(["headless", false, true]);
    const ask = ops().filter(({ op }) => op === "ask");
    expect(ask.map(({ payload }) => payload.text)).toEqual(["Which adapter should the reply use?"]);
    expect(ops().filter(({ op }) => op === "event").map(({ payload }) => payload.origin).filter(Boolean))
      .toEqual([{ kind: "unknown" }]);
    expect(first.lines[0]).toContain("headless: no terminal input");
    expect(first.lines.some((line) => line.includes("turn 1 ended"))).toBeTrue();
    expect(stdinListeners()).toBe(listenersBefore);
    const sessionId = first.host.sessionId();
    const sessionFile = first.host.session.sessionFile;
    if (!sessionFile) throw new Error("the bound session has no file");
    first.host.session.dispose();

    // When generation 2 resumes the recorded session headless and corral submits the answer:
    const second = await start(2, sessionFile);
    const requests = fixtureTransport().requests;
    await second.attach();
    expect([second.host.sessionId(), fixtureTransport().requests]).toEqual([sessionId, requests]);
    fixtureTransport().script([
      { toolCalls: [{ callId: "done-1", name: "corral_done", arguments: {
        summary: "Used the inbox adapter.\nVerified: headless fixture\nNot verified: none",
      } }] },
      { text: "DONE" },
    ]);
    const answered = nextEvent(second.events, (event) => event.kind === "turn_ended" && event.turn === 1);
    const submitted = await second.host.submit({
      text: "[corral answer] Use the inbox adapter.", busyPolicy: "prompt", idempotencyKey: "answer-1", origin: "inbox",
    }, "rq-answer-1");
    await answered;
    await second.host.drained();

    // Then the answer started a bridge turn in the same session, which reported done to corral.
    expect(submitted).toEqual({ disposition: "started", turn: 1 });
    const started = ops().filter(({ op, identity }) => op === "event" && identity.generation === 2)
      .map(({ payload }) => payload);
    expect(started[0]).toEqual({ kind: "turn_started", turn: 1, origin: { kind: "bridge", idempotency_key: "answer-1" } });
    const done = ops().filter(({ op }) => op === "done");
    expect(done.map(({ identity }) => identity.generation)).toEqual([2]);
    expect(second.host.state().mode).toBe("headless");
    expect(stdinListeners()).toBe(listenersBefore);
  } finally {
    for (const host of hosts) host.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
