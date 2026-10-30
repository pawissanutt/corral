// Native reviewer conversation continuity through actual host startup and offline transport.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../../bridge/events.ts";
import { openSession, withRecoveryContext } from "../../host/lifecycle.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession, authContext } from "../../host/session.ts";
import { CorralClient, sha256Hex } from "../../tools/corral.ts";
import { fixtureTransport } from "../../../test/fixtures/auth/index.ts";
import { writeFakeCorral } from "../../../test/support/fake-corral.ts";
import { PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { within } from "../../../test/support/wait.ts";
import type { HostIdentity, RoleContext } from "../types.ts";

const identity = {
  store: "st-0123456789abcdef", project: "demo", role: "reviewer" as const, task: "T-007", run: "R2",
  review: "R2-review1", callback: "g1:c1", generation: 1,
};

test("two native review children retain context, assess current candidates and do not repeat observed requests on recovery", async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-continued-review-"));
  const cwd = join(work, "worktree");
  const originalFetch = globalThis.fetch;
  mkdirSync(cwd);
  try {
    rmSync(join(assets.dir, "src/roles/reviewer"), { recursive: true, force: true });
    for (const tree of ["src/roles/reviewer", "src/roles/common", "assets"]) {
      cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
    }
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const directory = join(work, "continued-review");
    mkdirSync(directory);
    const fake = writeFakeCorral(directory);
    const sessionDir = join(directory, "sessions");

    const nonce = (review: string, generation: number) => sha256Hex(`${review}/${generation}`);
    async function create(bound: HostIdentity, manager: SessionManager, promptFile: string) {
      const loaded = withRecoveryContext(await loadModules(assets.dir, "reviewer"));
      let sent = 0;
      const modules = {
        ...loaded,
        role: {
          ...loaded.role,
          extend(pi: ExtensionAPI, context: RoleContext) {
            // Observe emissions while retaining the production handler and SDK input path.
            loaded.role.extend?.({
              ...pi,
              sendUserMessage(...args: Parameters<ExtensionAPI["sendUserMessage"]>) {
                sent++;
                pi.sendUserMessage(...args);
              },
            }, context);
          },
        },
      };
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch as typeof globalThis.fetch;
      const log = new EventLog(bound.generation);
      let current: HostSession | undefined;
      const client = new CorralClient({
        corral: fake.path, root: directory, identity: bound, hostCap: "h".repeat(64),
        sessionId: () => current?.sessionId() ?? "",
      });
      current = await HostSession.create({
        sdk, identity: bound, cwd, agentDir: join(directory, `${bound.review}-${bound.generation}`),
        model: "gpt-6.1-sol", promptFile, modules, client, events: log, network, sessionManager: manager,
      });
      await current.hello(nonce(bound.review!, bound.generation), `pane-${bound.review}`, sha256Hex("c".repeat(64)));
      return { host: current, events: log, kickoffs: () => sent };
    }

    let sessionId: string | undefined;
    let sessionFile: string | undefined;
    let recoverySnapshot: string | undefined;
    let promptFile = "";
    for (const round of [1, 2]) {
      const review = `R2-review${round}`;
      const bound: HostIdentity = { ...identity, review, callback: `gate${round}:${review}` };
      const candidate = String(round).repeat(40);
      promptFile = join(directory, `${review}.md`);
      writeFileSync(promptFile, `${candidate}\n`);
      const manager = openSession(sdk, cwd, sessionDir, 1, sessionFile);
      const previousUsers = manager.getEntries().filter((entry) =>
        entry.type === "message" && entry.message.role === "user").length;
      const actor = await create(bound, manager, promptFile);
      fixtureTransport().script([
        { toolCalls: [{ callId: `read-${round}`, name: "read", arguments: { path: promptFile } }] },
        { toolCalls: [{ callId: `verdict-${round}`, name: "corral_verdict",
          arguments: { verdict: round === 1 ? "changes" : "approve", notes: candidate } }] },
        { text: "FINISHED" },
      ]);
      let held: Promise<void> | undefined;
      if (round === 2) {
        fixtureTransport().holdNext();
        held = fixtureTransport().whenHeld();
      }
      let endTurn: (() => void) | undefined;
      const ended = new Promise<void>((resolve) => { endTurn = resolve; });
      const off = actor.events.on((event) => {
        if (event.kind === "turn_ended" && event.turn === 1) endTurn?.();
      });
      try {
        // Production binds extensions only after hello; subscribe before triggering startup.
        await actor.host.session.bindExtensions({});
        expect(actor.kickoffs()).toBe(1);
        if (held) {
          await within(held, 30_000, "the second review input to reach the fake transport");
          const file = actor.host.session.sessionFile;
          if (!file) throw new Error("review transcript missing");
          recoverySnapshot = readFileSync(file, "utf8");
          expect(manager.getEntries().filter((entry) =>
            entry.type === "message" && entry.message.role === "user").length).toBe(previousUsers + 1);
          fixtureTransport().release();
        }
        await within(ended, 30_000, `review ${round} verdict turn`);
        await actor.host.drained();
        expect(manager.getEntries().filter((entry) =>
          entry.type === "message" && entry.message.role === "user").length).toBe(previousUsers + 1);
        expect(actor.host.session.messages.some((message) =>
          message.role === "toolResult" && message.toolName === "read"
          && message.content.some((part) => part.type === "text" && part.text.includes(candidate)))).toBeTrue();
        if (sessionId !== undefined) expect(actor.host.sessionId()).toBe(sessionId);
        sessionId = actor.host.sessionId();
        sessionFile = actor.host.session.sessionFile;
        if (!sessionFile) throw new Error("review transcript missing");
      } finally {
        off();
        if (fixtureTransport().held) fixtureTransport().release();
        await actor.host.session.abort();
        await actor.host.drained();
        actor.host.session.dispose();
      }
    }

    const verdicts = fake.calls().map((call) => call.envelope).filter((envelope) => envelope.op === "verdict");
    expect(verdicts.map((call) => [call.identity.review, call.identity.callback, call.payload.notes])).toEqual([
      ["R2-review1", "gate1:R2-review1", "1".repeat(40)],
      ["R2-review2", "gate2:R2-review2", "2".repeat(40)],
    ]);
    expect(verdicts[0]?.request_id).not.toBe(verdicts[1]?.request_id);

    // Restore the actual in-flight snapshot: input observed, but this child's verdict not yet written.
    if (!sessionFile || recoverySnapshot === undefined) throw new Error("in-flight review snapshot missing");
    writeFileSync(sessionFile, recoverySnapshot);
    // A host crash for the same child must not resend that already observed input.
    const recovered = await create(
      { ...identity, review: "R2-review2", callback: "gate2:R2-review2", generation: 2 },
      openSession(sdk, cwd, sessionDir, 2, sessionFile), promptFile,
    );
    try {
      const requests = fixtureTransport().requests;
      await recovered.host.session.bindExtensions({});
      expect(recovered.host.sessionId()).toBe(sessionId);
      expect(recovered.kickoffs()).toBe(0);
      expect(fixtureTransport().requests).toBe(requests);
      expect(fake.calls().filter((call) => call.envelope.op === "verdict").length).toBe(2);
      const markers = readFileSync(sessionFile, "utf8").trim().split("\n").map((line) => JSON.parse(line))
        .filter((entry) => entry.type === "custom" && entry.customType === "corral_host_bound");
      expect(markers.map((entry) => [entry.data.generation, entry.data.session_id, entry.data.bind_nonce_sha256]))
        .toEqual([["R2-review1", 1], ["R2-review2", 1], ["R2-review2", 2]]
          .map(([review, generation]) => [generation, sessionId, sha256Hex(nonce(String(review), Number(generation)))]));
    } finally {
      await recovered.host.session.abort();
      await recovered.host.drained();
      recovered.host.session.dispose();
    }
  } finally {
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
