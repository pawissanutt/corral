// Offline production roles in real Pi sessions: loadout selection, receipt transport and denial.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { Role } from "../src/roles/types.ts";
import { CorralClient, sha256Hex, toolRequestId } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";
import { todoTool } from "../src/roles/common/todo.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { snapshotProgress } from "../src/ui/todo.ts";

for (const role of ["worker", "pm"] as const) {
  test(`${role} todo snapshot restores the canonical ledger through the read-only CLI`, async () => {
    // Given a saved ledger in the bound worker run or PM container.
    const work = mkdtempSync(join(tmpdir(), "corral-todo-snapshot-"));
    try {
      const progress = { revision: 7, spec_revision: "spec", phases: [] };
      const identity = { store: "s", project: "demo", role, task: "T-1", run: "R1", generation: 2 };
      const corral = writeFakeCorral(work);
      corral.snapshot(role === "worker" ? { runs: [{ id: "R1", progress }] } : { container: { todo: progress } });
      const client = new CorralClient({ corral: corral.path, root: work, identity,
        hostCap: "h".repeat(64), sessionId: () => "resumed-session" });
      // When a fresh host reads progress, without a model or a tool mutation.
      const value = await client.todoSnapshot();
      // Then it restores the ledger without sending a receipted operation.
      expect(snapshotProgress(value, identity)).toEqual(progress);
      expect(corral.calls()).toEqual([]);
    } finally {
      rmSync(work, { recursive: true, force: true });
    }
  });
}

const cases: readonly { role: Role; todo?: boolean }[] = [
  { role: "worker", todo: true }, { role: "pm", todo: true },
  { role: "worker", todo: false }, { role: "pm", todo: false },
  { role: "worker" }, { role: "pm" }, { role: "reviewer", todo: true },
];

for (const launch of cases) {
  test(`real ${launch.role} host enforces todo=${String(launch.todo)}`, async () => {
    // Given installed production roles, offline auth, and the launch snapshot:
    const assets = buildFixtureInstall();
    const work = mkdtempSync(join(tmpdir(), "corral-pi-todo-"));
    const originalFetch = globalThis.fetch;
    let host: HostSession | undefined;
    try {
      for (const tree of [`src/roles/${launch.role}`, "src/roles/common", "assets"]) {
        cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
      }
      const cwd = join(work, "cwd");
      mkdirSync(cwd);
      restrictEnvironment(process.env);
      const identity = {
        store: "st-demo", project: "demo", role: launch.role, generation: 1,
        ...(launch.role === "pm" ? {} : { task: "T-562", run: "R1" }),
      };
      const modules = await loadModules(assets.dir, launch.role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const sdk = await import("@earendil-works/pi-coding-agent");
      const events = new EventLog(1);
      const corral = writeFakeCorral(work);
      const client = new CorralClient({
        corral: corral.path, root: work, identity, hostCap: "h".repeat(64),
        sessionId: () => host?.sessionId() ?? "",
      });
      const enabled = launch.todo === true && launch.role !== "reviewer";
      const guardedModules = enabled ? modules : {
        ...modules,
        role: {
          ...modules.role,
          extend(pi: Parameters<NonNullable<typeof modules.role.extend>>[0], context: Parameters<typeof modules.role.tools>[0]) {
            modules.role.extend?.(pi, context);
            // A foreign extension can register a tool, but cannot expand the host's loadout.
            pi.registerTool(defineCorralTool(todoTool, {
              client, allowedOps: ["todo"], sessionId: () => host?.sessionId() ?? "",
              currentTurn: () => host?.currentTurn(),
            }));
          },
        },
      };
      host = await HostSession.create({
        sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
        ...launch, modules: guardedModules, client, events, network,
        sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
      });
      await host.session.bindExtensions({});
      await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
      expect(host.state().todo).toBe(enabled);
      expect(host.loadout.includes("corral_todo")).toBe(enabled);
      expect(host.session.getActiveToolNames().includes("corral_todo")).toBe(enabled);
      if (!enabled) {
        host.session.setActiveToolsByName([...host.loadout, "corral_todo"]);
      }
      // When the model calls the top-level tool:
      const queuedAttribution = launch.role === "pm" && launch.todo === true;
      const provider = fixtureTransport();
      const held = queuedAttribution ? provider.whenHeld() : undefined;
      if (queuedAttribution) provider.holdNext();
      provider.script([
        { toolCalls: [{ callId: "todo-call", name: "corral_todo", arguments: { op: "start", step: "implement" } }] },
        { text: "Finished" },
        ...(queuedAttribution ? [{ text: "Follow-up response" }] : []),
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      await host.submit({ text: "Update progress", busyPolicy: "prompt", idempotencyKey: "todo-input", origin: "test" }, "rq-todo");
      if (held) {
        await held;
        expect(await host.submit({ text: "Handle another item", busyPolicy: "follow_up",
          idempotencyKey: "todo-follow-up", origin: "test" }, "rq-follow-up")).toEqual({ disposition: "queued" });
        provider.release();
      }
      await ended;
      await host.drained();
      if (launch.role === "pm" && launch.todo === true) {
        const started = corral.calls().map(call => call.envelope)
          .find(call => call.op === "event" && call.payload.kind === "turn_started");
        expect(started).toBeDefined();
        const file = host.session.sessionFile;
        if (!file) throw new Error("PM session journal missing");
        const journal = readFileSync(file, "utf8").trim().split("\n").map(line => JSON.parse(line));
        const markers = journal.filter(entry => entry.type === "custom" && entry.customType === "corral_cost_turn");
        expect(markers.map(entry => entry.data)).toEqual([
          { generation: started?.identity.generation, turn: started?.payload.turn, phase: "start", key: "todo-input" },
          { generation: started?.identity.generation, turn: started?.payload.turn, phase: "start", key: "todo-follow-up", queued: true },
          { generation: started?.identity.generation, turn: started?.payload.turn, phase: "end" },
        ]);
        const response = journal.findIndex(entry => entry.type === "message" && entry.message.role === "assistant");
        const lastResponse = journal.findLastIndex(entry => entry.type === "message" && entry.message.role === "assistant");
        expect(response).toBeGreaterThan(journal.indexOf(markers[0]));
        const followed = journal.findIndex(entry => entry.type === "message" && entry.message.role === "assistant"
          && entry.message.content.some((part: { type: string; text?: string }) => part.type === "text" && part.text === "Follow-up response"));
        expect(followed).toBeGreaterThan(journal.indexOf(markers[1]));
        expect(journal.indexOf(markers[1])).toBeGreaterThan(response);
        expect(journal.indexOf(markers[2])).toBeGreaterThan(lastResponse);
      }
      // Then only enabled roles send the receipted operation, with bound identity and request ID.
      const calls = corral.calls().map((call) => call.envelope).filter((call) => call.op === "todo");
      expect(calls).toHaveLength(enabled ? 1 : 0);
      const toolCall = host.session.messages.flatMap((message) =>
        message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : [])[0];
      expect(toolCall?.name).toBe("corral_todo");
      if (enabled) {
        const call = calls[0];
        expect(call?.request_id).toBe(toolRequestId(host.sessionId(), toolCall?.id ?? ""));
        expect(call?.payload).toEqual({ op: "start", step: "implement" });
        expect(call?.identity.role).toBe(launch.role);
      } else {
        // The SDK's fixed allowlist removes even extension-registered tools before execution.
        expect(host.session.getActiveToolNames()).not.toContain("corral_todo");
        const result = host.session.messages.find((message) =>
          message.role === "toolResult" && message.toolCallId === toolCall?.id);
        expect(result?.role === "toolResult" && result.isError).toBeTrue();
      }
    } finally {
      host?.session.dispose();
      globalThis.fetch = originalFetch;
      assets.cleanup();
      rmSync(work, { recursive: true, force: true });
    }
  }, 60_000);
}
