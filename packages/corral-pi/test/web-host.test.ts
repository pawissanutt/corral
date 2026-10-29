import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { reviewerRole } from "../src/roles/reviewer/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import type { CapabilitySet, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const settings = { hosts: ["*"], max_bytes: 1024, timeout_ms: 5000, max_redirects: 3 };
const capabilities: CapabilitySet = {
  catalog: 2, digest: "a".repeat(64), preset: "standard", enabled: ["web.enabled"], web: settings,
};

test("worker and reviewer web calls use the same transport directly and nested across reload", async () => {
  // Given real role loadouts, a fake provider, and a public-address web wire fixture.
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-web-host-"));
  const originalFetch = globalThis.fetch;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    for (const roleName of ["worker", "reviewer"] as const) {
      const cwd = join(work, roleName);
      mkdirSync(cwd);
      const modules = await loadModules(assets.dir, roleName);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const requested: string[] = [];
      const network = guardedFetch(transport, undefined, "openai-codex", undefined, {
        settings, lookup: async () => [{ address: "93.184.216.34", family: 4 }],
        send: async (url) => { requested.push(String(url)); return new Response("public documentation"); },
      });
      globalThis.fetch = network.fetch;
      const role = roleName === "worker" ? workerRole(PACKAGE_ROOT) : reviewerRole(PACKAGE_ROOT);
      const nested: ToolDefinition = {
        name: "fixture_web", label: "Nested web read", description: "Exercises nested execution",
        parameters: Type.Object({}),
        async execute(_id, _params, signal, _update, ctx) {
          return (await ctx.executeTool("web_fetch", { url: "https://nested.example.org/" }, { signal })).result;
        },
      };
      const identity = { store: "st-demo", project: "demo", role: roleName, generation: 1, task: "T-568", run: "R1" };
      const events = new EventLog(1);
      const corral = writeFakeCorral(cwd);
      let host: HostSession | undefined;
      const client = new CorralClient({
        corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
      });
      host = await HostSession.create({
        sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", capabilities, client, events, network,
        modules: { ...modules, role: {
          ...role, localToolNames: [...role.localToolNames ?? [], nested.name],
          localTools: (context: RoleContext) => [...role.localTools?.(context) ?? [], nested],
        } },
        sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
      });
      try {
        await host.session.bindExtensions({});
        await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
        expect(host.state().capabilities).toEqual(capabilities);
        for (const stage of ["startup", "reload"]) {
          if (stage === "reload") await host.session.reload();
          // When the model issues direct and nested public reads, search, and a private read.
          fixtureTransport().script([
            { toolCalls: [{ callId: `${stage}-fetch`, name: "web_fetch", arguments: { url: "https://docs.example.org/" } }] },
            { toolCalls: [{ callId: `${stage}-search`, name: "web_search", arguments: { query: "docs" } }] },
            { toolCalls: [{ callId: `${stage}-nested`, name: nested.name, arguments: {} }] },
            { toolCalls: [{ callId: `${stage}-private`, name: "web_fetch", arguments: { url: "http://127.0.0.1/" } }] },
            { text: "Finished" },
          ]);
          const ended = nextEvent(events, (event) => event.kind === "turn_ended");
          expect((await host.submit({ text: stage, busyPolicy: "prompt", idempotencyKey: stage, origin: "test" },
            `rq-${stage}`)).disposition).toBe("started");
          await ended;
          await host.drained();
          // Then successful reads and setup reach the transcript; private requests fail locally.
          const results = host.session.messages.filter((message) =>
            message.role === "toolResult" && message.toolCallId.startsWith(`${stage}-`));
          expect(results.map((message) => message.role === "toolResult" && message.isError)).toEqual([false, false, false, true]);
          const search = results.find((message) => message.role === "toolResult" && message.toolName === "web_search");
          const text = search?.role === "toolResult" ? search.content.find((part) => part.type === "text") : undefined;
          expect(JSON.parse(text?.type === "text" ? text.text : "null").configured).toBeFalse();
        }
        expect(requested).toEqual([
          "https://docs.example.org/", "https://nested.example.org/", "https://docs.example.org/", "https://nested.example.org/",
        ]);
        expect(network.ledger().refused).toBe(2);
      } finally {
        host.session.dispose();
      }
    }
  } finally {
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
