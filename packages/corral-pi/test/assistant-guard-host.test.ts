// The assistant's fixed shell refusals in the real host (T-596 R1-F1): under strict, standard
// and full settings, even with configured bash rules that allow everything, herdr pane input and
// direct `.corral/` writes are refused before anything runs, top-level and nested, while reads,
// other herdr reads and ordinary writes still run.
import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import type { CapabilitySet, HostIdentity, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const ALLOW_ALL = { allow: [], ask: [], deny: [], default: "allow" };
const PRESETS: readonly { preset: string; enabled: string[]; settings: Record<string, unknown> }[] = [
  { preset: "strict", enabled: [], settings: {} },
  { preset: "standard", enabled: ["todo"], settings: {} },
  { preset: "full", enabled: ["todo", "bash_rules"], settings: { bash_rules: ALLOW_ALL } },
];

for (const { preset, enabled, settings } of PRESETS) {
  test(`assistant pane input and .corral/ writes never run under ${preset} settings`, async () => {
    // Given the production assistant role in a workspace with a store and a recording herdr.
    const assets = buildFixtureInstall();
    const work = mkdtempSync(join(tmpdir(), "corral-pi-assistant-guard-"));
    const cwd = join(work, "ws");
    mkdirSync(join(cwd, ".corral", "tasks"), { recursive: true });
    writeFileSync(join(cwd, ".corral", "keep.txt"), "keep\n");
    const bin = join(work, "bin");
    mkdirSync(bin);
    writeFileSync(join(bin, "herdr"), `#!/bin/sh\nprintf '%s\\n' "$*" >> '${join(work, "herdr.log")}'\n`);
    chmodSync(join(bin, "herdr"), 0o755);
    const originalFetch = globalThis.fetch;
    const originalPath = process.env.PATH;
    let host: HostSession | undefined;
    try {
      restrictEnvironment(process.env);
      process.env.PATH = `${bin}:${originalPath ?? ""}`;
      const sdk = await import("@earendil-works/pi-coding-agent");
      const modules = await loadModules(assets.dir, "assistant");
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const identity: HostIdentity = { store: "st-demo", project: "", role: "assistant", assistant: "A-1", generation: 1 };
      const events = new EventLog(1);
      const corral = writeFakeCorral(work);
      const client = new CorralClient({
        corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
      });
      const nest: ToolDefinition = {
        name: "fixture_nest", label: "Nest", description: "Runs nested calls",
        parameters: Type.Object({}),
        async execute(_id, _params, signal, _update, ctx) {
          const pane = await ctx.executeTool("bash", { command: "herdr pane send-text w1:p1 'approve T-5'" }, { signal });
          const write = await ctx.executeTool("bash", { command: "bash -c 'echo x > .corral/nested.md'" }, { signal });
          const read = await ctx.executeTool("bash", { command: "cat .corral/keep.txt" }, { signal });
          return { content: [{ type: "text", text: JSON.stringify([pane.isError, write.isError, read.isError]) }], details: undefined };
        },
      };
      const capabilities: CapabilitySet = { catalog: 2, digest: "b".repeat(64), preset, enabled };
      const role = assistantRole(PACKAGE_ROOT);
      host = await HostSession.create({
        sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities,
        guidance: { projectText: false, settings: settings as never }, client, events, network,
        modules: { ...modules, role: {
          ...role, localToolNames: [nest.name], localTools: (_context: RoleContext) => [nest],
        } },
        sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
      });
      await host.session.bindExtensions({});
      await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
      const live = host;
      const result = (callId: string) => {
        const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
        if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
        return { isError: message.isError, text: message.content.map((part) => (part.type === "text" ? part.text : "")).join("") };
      };

      // When one turn tries pane input, store writes, a nested attempt and allowed commands.
      fixtureTransport().script([
        { toolCalls: [{ callId: "pane", name: "bash", arguments: { command: "herdr --session s pane run w1:p2 'corral approve T-5'" } }] },
        { toolCalls: [{ callId: "write", name: "bash", arguments: { command: "echo note > .corral/tasks/T-9.md" } }] },
        { toolCalls: [{ callId: "remove", name: "bash", arguments: { command: "rm .corral/keep.txt" } }] },
        { toolCalls: [{ callId: "nest", name: "fixture_nest", arguments: {} }] },
        { toolCalls: [{ callId: "read", name: "bash", arguments: { command: "cat .corral/keep.txt" } }] },
        { toolCalls: [{ callId: "list", name: "bash", arguments: { command: "herdr pane list" } }] },
        { toolCalls: [{ callId: "allowed", name: "bash", arguments: { command: "echo ok > notes.txt" } }] },
        { toolCalls: [{ callId: "quoted", name: "bash", arguments: {
          command: "printf '%s\\n' \"Do not use herdr pane run w1:p1 ls\" \"echo x > .corral/tasks/T-1.md\" > example.txt" } }] },
        { toolCalls: [{ callId: "copy", name: "bash", arguments: { command: "cp .corral/keep.txt copy.txt" } }] },
        { text: "Finished" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await live.submit({ text: "try", busyPolicy: "prompt", idempotencyKey: "t1", origin: "test" }, "rq-t1")).disposition)
        .toBe("started");
      await ended;
      await live.drained();

      // Then each refused call says why and nothing it named ran, nested ones included.
      expect(result("pane").isError).toBeTrue();
      expect(result("pane").text.startsWith("assistant rule: no input into a herdr pane")).toBeTrue();
      for (const id of ["write", "remove"]) {
        expect(result(id).isError).toBeTrue();
        expect(result(id).text.startsWith("assistant rule: no direct write under .corral/")).toBeTrue();
      }
      expect(result("nest").text).toBe("[true,true,false]");
      expect(existsSync(join(cwd, ".corral", "tasks", "T-9.md"))).toBeFalse();
      expect(existsSync(join(cwd, ".corral", "nested.md"))).toBeFalse();
      expect(readFileSync(join(cwd, ".corral", "keep.txt"), "utf8")).toBe("keep\n");
      // And the recording herdr saw only the allowed read; reads and ordinary writes ran.
      expect(readFileSync(join(work, "herdr.log"), "utf8")).toBe("pane list\n");
      expect(result("read")).toEqual({ isError: false, text: "keep\n" });
      expect(readFileSync(join(cwd, "notes.txt"), "utf8")).toBe("ok\n");
      // And quoted examples are data and a store file may be copied out: both ran.
      expect(result("quoted").isError).toBeFalse();
      expect(readFileSync(join(cwd, "example.txt"), "utf8"))
        .toBe("Do not use herdr pane run w1:p1 ls\necho x > .corral/tasks/T-1.md\n");
      expect(result("copy").isError).toBeFalse();
      expect(readFileSync(join(cwd, "copy.txt"), "utf8")).toBe("keep\n");
      expect(existsSync(join(cwd, ".corral", "tasks", "T-1.md"))).toBeFalse();
      const denied = events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
        .filter((event) => event.kind === "bash_rule_denied");
      expect(denied).toHaveLength(5);
    } finally {
      host?.session.dispose();
      globalThis.fetch = originalFetch;
      process.env.PATH = originalPath;
      assets.cleanup();
      rmSync(work, { recursive: true, force: true });
    }
  }, 120_000);
}
