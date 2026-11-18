// Model-issued and ctx.executeTool calls share the role check across reload and refused session
// replacements. Code Mode itself cannot be nested inside another tool.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import type { CapabilitySet, RoleContext } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { expectReplacementsRefused } from "./support/replacement.ts";
import { nextEvent } from "./support/wait.ts";

type Outcome = { readonly name: string; readonly isError: boolean };

const capabilities: CapabilitySet = { catalog: 4, digest: "a".repeat(64), preset: "standard", enabled: ["codemode"] };

test("nested calls pass the role check and cannot recurse into Code Mode across reload and replacement", async () => {
  // Given the production worker with Code Mode, an orchestrating local tool in its loadout, and a
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
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-563", run: "R1" };
    const modules = await loadModules(assets.dir, "worker");
    const transport = fakeCodexTransport();
    const network = guardedFetch(transport.send);
    globalThis.fetch = network.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const events = new EventLog();
    const engine = writeFakeEngine(work);
    const client = new CorralClient({ corral: engine.path, root: work, cwd, identity });
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
      sdk: { ...sdk, createAgentSessionFromServices(opts) {
        return sdk.createAgentSessionFromServices({ ...opts, tools: [...opts.tools ?? [], hidden.name] });
      } }, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities,
      modules: wrapped, client, events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.session.bindExtensions({ onError: error => { throw error.error; } });
    await host.startLocal();
    expect(host.state().capabilities).toEqual(capabilities);
    expect(host.loadout).toContain("codemode");

    for (const stage of ["startup", "reload", "refused replacement"]) {
      if (stage === "reload") await host.session.reload();
      if (stage === "refused replacement") await expectReplacementsRefused(host, events);
      // When the model calls the orchestrator, then Code Mode and the foreign tool directly:
      transport.script([
        {
          toolCalls: [{
            callId: `${stage}-nest`, name: "fixture_nest", arguments: {
              calls: [
                { name: "read", args: { path: join(cwd, "notes.txt") } },
                { name: "codemode", args: { code: "throw new Error('nested cell ran');" } },
                { name: "fixture_hidden", args: {} },
              ],
            },
          }],
        },
        {
          toolCalls: [
            { callId: `${stage}-code`, name: "codemode", arguments: { code: "return 42;" } },
            { callId: `${stage}-hidden`, name: "fixture_hidden", arguments: {} },
          ],
        },
        { text: "Finished" },
      ]);
      const from = events.last;
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await host.submitLocal({ text: `Nest ${stage}`, busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await ended;
      await host.drained();
      // Nested reads are allowed and nested Code Mode is blocked before starting a cell.
      expect(outcomes.at(-1)).toEqual([
        { name: "read", isError: false }, { name: "codemode", isError: true }, { name: "fixture_hidden", isError: true },
      ]);
      expect(hiddenRuns).toBe(0);
      const page = events.since(from, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events;
      expect(page.filter(event => event.kind === "tool_denied").map(event => [event.tool, event.nested]))
        .toEqual([["fixture_hidden", true]]);
      expect(page.filter(event => event.kind === "code_cell" && event.phase === "submitted")).toHaveLength(1);
      const cell = host.session.messages.find(message => message.role === "toolResult" &&
        message.toolCallId.startsWith(`${stage}-code|`));
      if (cell?.role !== "toolResult") throw new Error("direct Code Mode result missing");
      expect(cell.isError).toBeFalse();
      expect(cell.content.some(part => part.type === "text" && part.text.includes("Return value: 42"))).toBeTrue();
      const call = host.session.messages.flatMap((message) =>
        message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : [])
        .filter((part) => part.name === "fixture_hidden").at(-1);
      const direct = host.session.messages.find((message) =>
        message.role === "toolResult" && message.toolCallId === call?.id);
      expect(direct?.role === "toolResult" && direct.isError).toBeTrue();
      expect(host.session.getActiveToolNames()).not.toContain("fixture_hidden");
    }
  } finally {
    await host?.close();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
