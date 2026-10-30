import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

for (const mode of ["settlement-limit", "changes-after-done"] as const) test(`host budget: ${mode}`, async () => {
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "budget-host-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("missing fixture transport");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-619", run: "R1" };
    const events = new EventLog(1);
    const corral = writeFakeCorral(cwd);
    const path = corral.path;
    // A run closes after done: a subsequent reservation gets run_ended; review changes reopen it.
    // The other fixture exhausts exactly on a successful settlement, not on an error message.
    let script = readFileSync(path, "utf8");
    script = script.replace('state: "committed",', `state: envelope.payload.kind === "budget" && envelope.payload.action === "reserve" && existsSync(join(dir, "reject-reserve")) ? "rejected" : "committed",`);
    script = script.replace('{ admitted: true, exhausted: false }', `{ admitted: true, exhausted: envelope.payload.action === "settle" && existsSync(join(dir, "exhaust")), tokens: 6, limits: { max_tokens: existsSync(join(dir, "exhaust")) ? 6 : null, max_usd_nanos: null } }`);
    writeFileSync(path, script);
    const client = new CorralClient({ corral: path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", modules, client, events, network,
      capabilities: { catalog: 7, digest: "a".repeat(64), preset: "standard", enabled: [] },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")) });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const turn = async (n: number) => {
      const ended = nextEvent(events, (e) => e.kind === "turn_ended" && e.turn === n);
      await host!.submit({ text: `Reply only TURN${n}.`, busyPolicy: "prompt", idempotencyKey: `k${n}`, origin: "test" }, `rq${n}`);
      await ended;
      await host!.drained();
    };
    if (mode === "settlement-limit") {
      writeFileSync(join(cwd, "exhaust"), "yes");
      await turn(1);
      const lifecycle = corral.calls().filter((c) => c.envelope.op === "event").map((c) => c.envelope.payload);
      const stopped = lifecycle.filter((p) => p.kind === "budget_exhausted");
      expect(stopped).toHaveLength(1);
      const budget = stopped[0]?.budget as { stopped: boolean; exhausted: boolean; tokens: number };
      expect([budget.stopped, budget.exhausted, budget.tokens]).toEqual([true, true, 6]);
      expect(lifecycle.findIndex((p) => p.kind === "budget_exhausted") < lifecycle.findIndex((p) => p.kind === "turn_ended")).toBeTrue();
      expect(lifecycle.filter((p) => p.kind === "provider_error")).toHaveLength(0);
    } else {
      await turn(1); // Earlier accounted usage.
      writeFileSync(join(cwd, "reject-reserve"), "yes");
      await turn(2); // Reservation after done is refused as run_ended.
      rmSync(join(cwd, "reject-reserve"));
      await turn(3); // Same host receives review changes and admits another request.
      const lifecycle = corral.calls().filter((c) => c.envelope.op === "event").map((c) => c.envelope.payload);
      expect(lifecycle.filter((p) => p.kind === "budget" && p.action === "reserve")).toHaveLength(3);
      expect(lifecycle.filter((p) => p.kind === "budget_exhausted")).toHaveLength(0);
      expect((host.state().budget as { stopped: boolean }).stopped).toBeFalse();
      const last = host.session.messages.at(-1);
      expect(last?.role === "assistant" && last.stopReason === "stop").toBeTrue();
    }
  } finally {
    await host?.close(); globalThis.fetch = originalFetch; assets.cleanup(); rmSync(cwd, { recursive: true, force: true });
  }
}, 30_000);
