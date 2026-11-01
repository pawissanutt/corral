// Real QuickJS scripts, deterministic provider fixtures, no inference or host load.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HostSessionOptions } from "../src/host/session.ts";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

for (const role of ["worker", "reviewer", "pm"] as const) {
  test(`${role} codemode loadout, static cost and guarded scripts across reload`, async () => {
    const assets = buildFixtureInstall();
    const work = mkdtempSync(join(tmpdir(), "corral-codemode-"));
    const originalFetch = globalThis.fetch;
    let host: HostSession | undefined;
    try {
      for (const tree of [`src/roles/${role}`, "src/roles/common", "src/extensions", "assets"]) {
        cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
      }
      const cwd = join(work, "cwd");
      mkdirSync(cwd);
      const source = `${"discard this line\n".repeat(100)}keep: one\nkeep: two\n`;
      writeFileSync(join(cwd, "one.txt"), source);
      writeFileSync(join(cwd, "two.txt"), source);
      restrictEnvironment(process.env);
      const identity = { store: "st-demo", project: "demo", role, generation: 1, task: "T-566", run: "R1" };
      const modules = await loadModules(assets.dir, role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const sdk = await import("@earendil-works/pi-coding-agent");
      const events = new EventLog(1);
      const corral = writeFakeCorral(work);
      let forbiddenRuns = 0;
      // Callable upstream but NOT in this role's loadout: the common check must reject it.
      const forbidden: ToolDefinition = {
        name: "fixture_forbidden_write", label: "Forbidden", description: "Outside the role loadout",
        exposure: "codemode", parameters: Type.Object({}),
        async execute() {
          forbiddenRuns++;
          return { content: [{ type: "text", text: "must never run" }], details: undefined };
        },
      };
      const wrapped = { ...modules, role: { ...modules.role,
        extend(pi: Parameters<NonNullable<typeof modules.role.extend>>[0]) { pi.registerTool(forbidden); },
      } };
      const capabilities: CapabilitySet = { catalog: 4, digest: "a".repeat(64), preset: role === "pm" ? "full" : "standard", enabled: ["codemode"] };
      const options: HostSessionOptions = {
        // Simulate a late/foreign extension making a tool callable upstream. Host policy
        // still excludes it, so the common execution check (not discovery) must refuse it.
        sdk: { ...sdk, createAgentSessionFromServices(options) {
          return sdk.createAgentSessionFromServices({ ...options, tools: [...options.tools ?? [], forbidden.name] });
        } }, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities,
        modules: wrapped, events, network,
        client: new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64),
          sessionId: () => host?.sessionId() ?? "" }),
        sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
      };
      host = await HostSession.create(options);
      await host.session.bindExtensions({});
      await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
      expect(host.loadout).toContain("codemode");
      const metadata = () => JSON.stringify(host?.session.agent.state.tools.map(({ name, description, parameters }) =>
        ({ name, description, parameters })));
      const enabledBytes = Buffer.byteLength(metadata());
      const enabledPrompt = Buffer.byteLength(host.session.systemPrompt);
      host.session.setActiveToolsByName(host.loadout.filter(name => name !== "codemode"));
      const toolDelta = enabledBytes - Buffer.byteLength(metadata());
      console.log(`CODEMODE_STATIC_COST ${JSON.stringify({
        estimator: "utf8-json-bytes/4-v1", role, tool_bytes_delta: toolDelta,
        tool_tokens_estimate_delta: Math.ceil(toolDelta / 4),
        prompt_bytes_delta: enabledPrompt - Buffer.byteLength(host.session.systemPrompt),
      })}`);
      host.session.setActiveToolsByName([...host.loadout]);
      for (const stage of ["startup", "reload"]) {
        if (stage === "reload") await host.session.reload();
        const code = `
          // Discovery forces the complete callable tool catalog, including the forbidden fixture.
          const catalog = ALL_TOOLS;
          text("catalog: " + catalog.filter(t => t.name.startsWith("corral_")).map(t => t.name).join(","));
          ${role === "pm" ? `
          const reads = await Promise.all(["corral_brief", "corral_next", "corral_inbox_read"].map(name => tools[name]({})));
          text(reads);
          ` : `
          const results = await Promise.all(["one.txt", "two.txt"].map(path => tools.read({path})));
          text(results.map(s => s.split("\\n").filter(line => line.startsWith("keep:")).join("\\n")));
          `}
          const denied = await Promise.allSettled(["fixture_forbidden_write", "${role === "pm" ? "corral_comment" : role === "reviewer" ? "corral_verdict" : "corral_done"}"].map(async name => await tools[name]({})));
          text("raw rejection: " + JSON.stringify(denied[0].reason));
          text(denied.map(r => r.status === "rejected" ? {status: r.status, error: String(r.reason?.message ?? r.reason)} : "UNEXPECTED execution"));
          text("models: " + typeof models);
          for (const name of ["classify", "generateImages"]) {
            try { await models[name]({provider: "openai", id: "fake"}, {}); text("UNEXPECTED model"); }
            catch { text(name + ": disabled"); }
          }
        `;
        fixtureTransport().script([
          { toolCalls: [{ callId: `${stage}-script`, name: "codemode", arguments: { code } }] },
          { text: "Finished" },
        ]);
        const ended = nextEvent(events, (event) => event.kind === "turn_ended");
        await host.submit({ text: stage, busyPolicy: "prompt", idempotencyKey: stage, origin: "test" }, `rq-${stage}`);
        await ended;
        await host.drained();
        const result = host.session.messages.find(message => message.role === "toolResult" && message.toolCallId.startsWith(`${stage}-script|`));
        if (result?.role !== "toolResult") throw new Error("missing codemode result");
        expect(result.isError).not.toBeTrue();
        const output = result.content.filter(part => part.type === "text").map(part => part.text).join("\n");
        expect(output).toContain("Script completed");
        if (role === "pm") {
          expect(output).toContain("read_brief");
          expect(output).toContain("read_next");
          expect(output).toContain("inbox_read");
          const reads = corral.calls().filter(({ envelope }) => ["read_brief", "read_next", "inbox_read"].includes(envelope.op));
          expect(reads).toHaveLength(stage === "startup" ? 3 : 6);
          for (const { envelope } of reads) {
            expect(envelope.identity).toMatchObject({ ...identity, session_id: host.sessionId() });
            expect(envelope.auth.host_cap).toBe("h".repeat(64));
            expect(envelope.payload).toEqual({});
          }
        } else {
          expect(output).toContain("keep: one");
          expect(output).not.toContain("discard this line");
          expect(output.length < source.length).toBeTrue();
          expect(result.nestedCalls?.calls.filter(call => call.name === "read")).toHaveLength(2);
        }
        expect(output).toContain("fixture_forbidden_write");
        expect(output).toContain("not in this session's loadout");
        expect(output).toContain("raw rejection: {}");
        expect(output).toContain('"status":"rejected"');
        const catalog = output.split("\\n").find(line => line.startsWith("catalog:")) ?? "";
        expect(catalog).not.toMatch(/corral_(done|verdict|comment|todo|inbox_resolve|ask|gate)/);
        if (role !== "pm") expect(catalog).not.toContain("corral_brief");
        expect(output).toContain("models: undefined");
        expect(output).toContain("classify: disabled");
        expect(output).toContain("generateImages: disabled");
        expect(output).not.toContain("UNEXPECTED");
        expect(forbiddenRuns).toBe(0);
        expect(corral.calls().some(({ envelope }) => ["done", "verdict", "comment"].includes(envelope.op))).toBeFalse();
        const denials = events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
          .filter(event => event.kind === "tool_denied" && event.nested === true);
        expect(denials.some(event => event.tool === forbidden.name)).toBeTrue();
        // model-only operations are absent from discovery and refused by upstream before execution.
        expect(output).not.toContain('"error":{}');
      }
      await host.close();
      host = await HostSession.create({ ...options, capabilities: { ...capabilities, preset: "strict", enabled: [] },
        sessionManager: sdk.SessionManager.inMemory(cwd) });
      await host.session.bindExtensions({});
      for (const stage of ["disabled", "disabled reload"]) {
        if (stage === "disabled reload") await host.session.reload();
        expect(host.loadout).not.toContain("codemode");
        expect(host.session.getAllTools().some(tool => tool.name === "codemode")).toBeFalse();
      }
    } finally {
      await host?.close();
      globalThis.fetch = originalFetch;
      assets.cleanup();
      rmSync(work, { recursive: true, force: true });
    }
  }, 60_000);
}
