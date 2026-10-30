// Offline production roles in real Pi sessions: loadout selection, receipt transport and denial.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
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
      fixtureTransport().script([
        { toolCalls: [{ callId: "todo-call", name: "corral_todo", arguments: { op: "start", step: "implement" } }] },
        { text: "Finished" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      await host.submit({ text: "Update progress", busyPolicy: "prompt", idempotencyKey: "todo-input", origin: "test" }, "rq-todo");
      await ended;
      await host.drained();
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
