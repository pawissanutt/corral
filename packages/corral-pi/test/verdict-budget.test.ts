import { expect, test } from "bun:test";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

for (const role of ["reviewer", "plan_reviewer", "worker"] as const) {
  for (const accepted of [true, false]) {
    test(`${role}: ${accepted ? "committed" : "refused"} close controls model continuation`, async () => {
      const assets = buildFixtureInstall({ omit: ["role"] });
      cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
      const cwd = mkdtempSync(join(tmpdir(), "verdict-budget-"));
      const originalFetch = globalThis.fetch;
      let host: HostSession | undefined;
      try {
        const modules = await loadModules(assets.dir, role);
        const transport = fakeCodexTransport();
        const network = guardedFetch(transport.send);
        globalThis.fetch = network.fetch;
        const identity = { store: "st-demo", project: "demo", role, generation: 1,
          task: "T-007", run: role === "plan_reviewer" ? "P1" : "R1", review: "review1" };
        const events = new EventLog(1);
        const corral = writeFakeCorral(cwd);
        const client = new CorralClient({ corral: corral.path, root: cwd, identity,
          hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
        const call = client.call.bind(client);
        let closed = false;
        let lateReservations = 0;
        const verdictOp = role === "reviewer" ? "verdict" : role === "plan_reviewer" ? "plan_verdict" : "done";
        client.call = async (op, args) => {
          const receipt = await call(op, args);
          if (op === verdictOp) {
            closed = accepted;
            if (!accepted) return { ...receipt, state: "rejected", result: { code: "pending_questions" } };
          }
          if (op === "event" && args.payload?.kind === "budget" && args.payload.action === "reserve" && closed) {
            lateReservations++;
            return { ...receipt, state: "rejected", result: { code: role === "worker" ? "run_ended" : "review_closed", reason: "closed" } };
          }
          return receipt;
        };
        host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"),
          model: "gpt-6.1-sol", modules, client, events, network,
          capabilities: { catalog: 7, digest: "a".repeat(64), preset: "standard", enabled: ["role_enabled"] },
          sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")) });
        await host.session.bindExtensions({});
        await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
        transport.script([
          { toolCalls: [{ callId: "verdict", name: `corral_${verdictOp}`,
            arguments: role === "worker" ? { summary: "Completed.\nVerified: tests\nNot verified: none" }
              : { verdict: "approve", notes: "Checked the candidate." } }] },
          { text: "The verdict was refused; resolve the pending question." },
        ]);
        const ended = nextEvent(events, (event) => event.kind === "turn_ended");
        await host.submit({ text: "Record the verdict.", busyPolicy: "prompt", idempotencyKey: "verdict", origin: "test" }, "rq-verdict");
        await ended;
        await host.drained();
        expect(corral.calls().filter((c) => c.envelope.op === verdictOp)).toHaveLength(1);
        expect(lateReservations).toBe(0);
        const errors = host.session.messages.filter((message) => message.role === "assistant" && message.stopReason === "error");
        expect(errors).toHaveLength(0);
        const reserves = corral.calls().filter((c) => c.envelope.payload.kind === "budget" && c.envelope.payload.action === "reserve");
        expect(reserves).toHaveLength(accepted ? 1 : 2);
        if (!accepted) expect(host.session.getLastAssistantText()).toContain("verdict was refused");
      } finally {
        await host?.close();
        globalThis.fetch = originalFetch;
        assets.cleanup();
        rmSync(cwd, { recursive: true, force: true });
      }
    }, 30_000);
  }
}
