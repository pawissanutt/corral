// One journey through production HostSession/SDK: clarification -> independent read -> resume ->
// owned UI answer -> exactly one attributed follow-up -> dependent step. No real provider or pane.
import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Component } from "@earendil-works/pi-tui";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { QUESTION_ENTRY } from "../src/host/pending-questions.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";

test("deferred clarification leaves composer available, survives resume and delivers one real owner answer", async () => {
  const assets = buildFixtureInstall(), work = mkdtempSync(join(tmpdir(), "ask-deferred-owned-"));
  const originalFetch = globalThis.fetch, originalEnv = { ...process.env };
  const fake = writeFakeCorral(work);
  let host: HostSession | undefined, widget: Component | undefined;
  const statuses = new Map<string, string | undefined>(), selections: string[] = [], notices: string[] = [];
  // This UI belongs only to this fixture. No terminal input, user pane, or modal is opened by ask.
  const ui = {
    setStatus: (key: string, text: string | undefined) => { statuses.set(key, text); },
    setWidget: (_key: string, factory: (tui: unknown, theme: unknown) => Component) => {
      widget = factory({ requestRender() {} }, { fg: (_c: string, text: string) => text });
    },
    notify: (text: string) => { notices.push(text); },
    select: async (title: string, options: string[]) => { selections.push(title); return options[0]; },
    input: async () => "Only the small fix",
  } as unknown as ExtensionUIContext;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("missing fixture transport");
    const network = guardedFetch(transport); globalThis.fetch = network.fetch;
    const create = async (generation: number, file?: string) => {
      const identity = { store: "st-demo", project: "", role: "assistant" as const, assistant: "A-1", generation };
      const client = new CorralClient({ corral: fake.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
      host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
        capabilities: { catalog: 8, digest: "b".repeat(64), preset: "standard", enabled: ["ask_user"] },
        ui: "plain", client, events: new EventLog(generation), network,
        modules: { ...modules, role: assistantRole(PACKAGE_ROOT) },
        sessionManager: file ? sdk.SessionManager.open(file) : sdk.SessionManager.create(work, join(work, "sessions")) });
      await host.session.bindExtensions({ mode: "tui", uiContext: ui });
      await host.hello("n".repeat(64), "owned-test-pane", sha256Hex("c".repeat(64)));
      return host;
    };
    let live = await create(1);
    const file = live.session.sessionFile!;
    const unrelated = join(work, "research.txt"); writeFileSync(unrelated, "independent research completed");
    fixtureTransport().script([
      { toolCalls: [{ callId: "clarify", name: "ask_user", arguments: { nonBlocking: true, key: "scope",
        questions: [{ question: "Which scope?", options: ["Small (recommended)", "Large"], freeText: true }] } }] },
      { toolCalls: [{ callId: "independent", name: "read", arguments: { path: unrelated } }] },
      { text: "Research is complete; only the scope-dependent step waits." },
    ]);
    await live.session.prompt("Clarify the scope using choices while researching the fixture", { source: "extension" });
    await live.drained();
    const messages = live.session.messages;
    const result = messages.find(m => m.role === "toolResult" && m.toolName === "ask_user");
    expect(result?.role === "toolResult" ? result.details : null).toMatchObject({ status: "pending" });
    expect(messages.some(m => m.role === "toolResult" && m.toolName === "read" && JSON.stringify(m).includes("independent research completed"))).toBeTrue();
    expect(selections).toEqual([]);
    expect(fake.calls().filter(c => c.envelope.payload.kind === "owner_input")).toHaveLength(0);
    expect(statuses.get("corral-owner-questions")).toContain("1 owner question");
    expect(widget?.render(100).join("\n")).toContain("/questions");
    const q = live.session.sessionManager.getEntries().findLast(e => e.type === "custom" && e.customType === QUESTION_ENTRY);
    const id = q?.type === "custom" ? (q.data as { id: string }).id : "missing";
    await live.close();
    live = await create(2, file);
    expect(statuses.get("corral-owner-questions")).toContain("1 owner question");
    expect(widget?.render(100).join("\n")).toContain(id);
    fixtureTransport().script([
      { toolCalls: [{ callId: "dependent", name: "read", arguments: { path: unrelated } }] },
      { text: "The owner selected the small fix; dependent work resumed." },
    ]);
    await live.session.prompt(`/questions ${id}`, { source: "interactive" });
    await live.session.waitForIdle(); await live.drained();
    const owner = fake.calls().filter(c => c.envelope.payload.kind === "owner_input");
    expect(owner).toHaveLength(1);
    expect(owner[0]!.envelope.payload).toMatchObject({ source: "dialog" });
    expect(owner[0]!.envelope.payload.text).toContain(`[ask_user answer ${id}]`);
    expect(owner[0]!.envelope.payload.text).toContain("Which scope?: Small (recommended)\nOnly the small fix");
    const userAnswers = () => live.session.messages.filter(m => m.role === "user" && JSON.stringify(m).includes(`[ask_user answer ${id}]`));
    expect(userAnswers()).toHaveLength(1);
    expect(JSON.stringify(userAnswers())).toContain("[owner input A-1/g2/i");
    expect(live.session.messages.some(m => m.role === "toolResult" && m.toolCallId.startsWith("dependent"))).toBeTrue();
    await live.session.prompt(`/questions ${id}`, { source: "interactive" });
    expect(selections).toEqual(["Which scope?"]);
    expect(fake.calls().filter(c => c.envelope.payload.kind === "owner_input")).toHaveLength(1);
    await live.close(); live = await create(3, file);
    expect(userAnswers()).toHaveLength(1);
    expect(fake.calls().filter(c => c.envelope.payload.kind === "owner_input")).toHaveLength(1);
    expect(statuses.get("corral-owner-questions")).toBeUndefined();
    expect(notices.some(n => n.includes("not delivered"))).toBeFalse();
  } finally {
    await host?.close(); globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv); assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 30_000);
