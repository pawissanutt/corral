// The capability framework's nested guard smoke (T-563): model-issued and nested
// (`ctx.executeTool`) calls pass the host's one execution check, before and after a reload and
// after every refused session replacement. A corral operation runs only at top level.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { expectReplacementsRefused } from "./support/replacement.ts";
import { nextEvent } from "./support/wait.ts";

type Outcome = { readonly name: string; readonly isError: boolean };

const capabilities: CapabilitySet = { catalog: 1, digest: "a".repeat(64), preset: "standard", enabled: ["todo"] };

test("nested calls pass the role check and corral operations stay top-level across reload and replacement", async () => {
  // Given the production worker with todo, an orchestrating local tool in its loadout, and a
  // foreign callable tool outside it:
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-nested-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    for (const tree of ["src/roles/worker", "src/roles/common", "src/extensions", "assets"]) {
      cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
    }
    const cwd = join(work, "cwd");
    mkdirSync(cwd);
    writeFileSync(join(cwd, "notes.txt"), "readable\n");
    restrictEnvironment(process.env);
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-563", run: "R1" };
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const events = new EventLog(1);
    const corral = writeFakeCorral(work);
    const client = new CorralClient({
      corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    const outcomes: Outcome[][] = [];
    let hiddenRuns = 0;
    const nest: ToolDefinition = {
      name: "fixture_nest", label: "Nest", description: "Runs the named tools through ctx.executeTool.",
      parameters: Type.Object({ calls: Type.Array(Type.Object({ name: Type.String(), args: Type.Any() })) }),
      async execute(_id, params, signal, _update, ctx) {
        const results: Outcome[] = [];
        for (const call of (params as { calls: { name: string; args: unknown }[] }).calls) {
          const outcome = await ctx.executeTool(call.name, call.args, { signal });
          results.push({ name: call.name, isError: outcome.isError });
        }
        outcomes.push(results);
        return { content: [{ type: "text", text: JSON.stringify(results) }], details: undefined };
      },
    };
    const hidden: ToolDefinition = {
      name: "fixture_hidden", label: "Hidden", description: "Callable by other tools whenever registered.",
      parameters: Type.Object({}), exposure: "codemode",
      async execute() {
        hiddenRuns++;
        return { content: [{ type: "text", text: "ran" }], details: undefined };
      },
    };
    const role = modules.role;
    const wrapped = {
      ...modules,
      role: {
        ...role,
        localToolNames: [...(role.localToolNames ?? []), nest.name],
        localTools: (context: RoleContext) => [...(role.localTools?.(context) ?? []), nest],
        extend(pi: Parameters<NonNullable<typeof role.extend>>[0], context: RoleContext) {
          role.extend?.(pi, context);
          pi.registerTool(hidden);
        },
      },
    };
    host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities,
      modules: wrapped, client, events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    expect(host.state().capabilities).toEqual(capabilities);
    expect(host.loadout).toContain("corral_todo");

    for (const stage of ["startup", "reload", "refused replacement"]) {
      if (stage === "reload") await host.session.reload();
      if (stage === "refused replacement") await expectReplacementsRefused(host, events);
      // When the model calls the orchestrator, then corral_todo and the foreign tool directly:
      fixtureTransport().script([
        {
          toolCalls: [{
            callId: `${stage}-nest`, name: "fixture_nest", arguments: {
              calls: [
                { name: "read", args: { path: join(cwd, "notes.txt") } },
                { name: "corral_todo", args: { op: "start", step: "nested" } },
                { name: "fixture_hidden", args: {} },
              ],
            },
          }],
        },
        {
          toolCalls: [
            { callId: `${stage}-todo`, name: "corral_todo", arguments: { op: "start", step: stage } },
            { callId: `${stage}-hidden`, name: "fixture_hidden", arguments: {} },
          ],
        },
        { text: "Finished" },
      ]);
      const before = corral.calls().length;
      const from = events.last;
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await host.submit({ text: `Nest ${stage}`, busyPolicy: "prompt", idempotencyKey: stage, origin: "test" },
        `rq-${stage}`)).disposition).toBe("started");
      await ended;
      await host.drained();
      // Then the nested read runs, the nested corral call is refused by the host's check, the
      // foreign tool is unreachable from either level, and only the top-level todo reaches corral.
      expect(outcomes.at(-1)).toEqual([
        { name: "read", isError: false }, { name: "corral_todo", isError: true }, { name: "fixture_hidden", isError: true },
      ]);
      expect(hiddenRuns).toBe(0);
      const todo = corral.calls().slice(before).map(({ envelope }) => envelope).filter(({ op }) => op === "todo");
      expect(todo.map(({ payload }) => payload)).toEqual([{ op: "start", step: stage }]);
      const denied = events.since(from, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
        .filter((event) => event.kind === "tool_denied")
        .map((event) => ({ tool: event.tool, nested: event.nested === true }));
      expect(denied).toEqual([{ tool: "corral_todo", nested: true }]);
      const call = host.session.messages.flatMap((message) =>
        message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : [])
        .filter((part) => part.name === "fixture_hidden").at(-1);
      const direct = host.session.messages.find((message) =>
        message.role === "toolResult" && message.toolCallId === call?.id);
      expect(direct?.role === "toolResult" && direct.isError).toBeTrue();
      expect(host.session.getActiveToolNames()).not.toContain("fixture_hidden");
    }
  } finally {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
