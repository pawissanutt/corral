// Exercise the production HostSession and roles with deterministic provider/UI boundaries.
import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { pmRole } from "../src/roles/pm/index.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";

for (const role of ["assistant", "pm"] as const) {
  test(`${role} host records completed dialog answers as user messages and not cancellations`, async () => {
    const assets = buildFixtureInstall();
    const work = mkdtempSync(join(tmpdir(), `ask-user-${role}-`));
    const cwd = join(work, "ws");
    mkdirSync(cwd, { recursive: true });
    const originalFetch = globalThis.fetch;
    const originalEnv = { ...process.env };
    let host: HostSession | undefined;
    try {
      restrictEnvironment(process.env);
      const sdk = await import("@earendil-works/pi-coding-agent");
      const modules = await loadModules(assets.dir, role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const identity: HostIdentity = { store: "st-demo", project: role === "pm" ? "demo" : "", role,
        ...(role === "assistant" ? { assistant: "A-1" } : {}) };
      const client = new CorralClient({ corral: "", root: work, cwd, identity });
      host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
        capabilities: { catalog: 8, digest: "b".repeat(64), preset: "standard", enabled: ["ask_user"] },
        ui: "plain", runtimeDir: work, client, events: new EventLog(), network,
        modules: { ...modules, role: role === "assistant" ? assistantRole(PACKAGE_ROOT) : pmRole(PACKAGE_ROOT) },
        sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
      let cancel = false;
      const seen: unknown[] = [];
      const ui = { select: async (title: string, options: string[]) => {
        seen.push([title, options]); return cancel ? undefined : "Two";
      }, input: async () => "Extra detail", setStatus: () => {} };
      await host.session.bindExtensions({ mode: "tui", uiContext: ui as unknown as ExtensionUIContext });
      await host.startLocal();
      const live = host;
      const userText = () => live.session.messages.filter((m) => m.role === "user")
        .map((m) => typeof m.content === "string" ? m.content : m.content.map((p) => p.type === "text" ? p.text : "").join(""));
      const ask = (id: string) => ({ toolCalls: [{ callId: id, name: "ask_user", arguments: {
        questions: [{ question: "Which?", options: ["One", "Two"], freeText: true }],
      } }] });
      // The recorded answer steers in beside the tool result, so one reply reads both.
      fixtureTransport().script([ask("answer"), { text: "Understood." }]);
      await live.session.prompt("Ask a choice", { source: "extension" });
      await live.drained();
      expect(live.state().last_error).toBeNull();
      const answer = "Which?: Two\nExtra detail";
      expect(seen).toEqual([["Which?", ["One", "Two", "Other (type an answer)"]]]);
      expect(userText().filter(text => text === answer)).toEqual([answer]);
      expect(JSON.stringify(live.session.sessionManager.getEntries())).toContain(answer.replaceAll("\n", "\\n"));
      const before = userText();
      cancel = true;
      fixtureTransport().script([ask("cancel"), { text: "No answer." }]);
      await live.session.prompt("Ask again", { source: "extension" });
      await live.drained();
      expect(userText().slice(before.length)).toEqual(["Ask again"]);
      const result = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.startsWith("cancel"));
      expect(result).toMatchObject({ details: { status: "no answer" } });
    } finally {
      await host?.close();
      globalThis.fetch = originalFetch;
      for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
      Object.assign(process.env, originalEnv);
      assets.cleanup();
      rmSync(work, { recursive: true, force: true });
    }
  }, 120_000);
}
