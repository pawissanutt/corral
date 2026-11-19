// One journey through production HostSession/SDK: clarification -> independent read -> resume ->
// pane answer -> exactly one owner follow-up -> dependent step. No real provider or pane.
import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Component } from "@earendil-works/pi-tui";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { EVENT_PAGE_BYTES } from "../src/bridge/protocol.ts";
import { loadModules } from "../src/host/modules.ts";
import { answerMarker, QUESTION_ENTRY } from "../src/host/pending-questions.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";

test("deferred clarification leaves composer available, survives resume and delivers one real owner answer", async () => {
  const assets = buildFixtureInstall(), work = mkdtempSync(join(tmpdir(), "ask-deferred-owned-"));
  const originalFetch = globalThis.fetch, originalEnv = { ...process.env };
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
    const transport = fakeCodexTransport();
    const network = guardedFetch(transport.send); globalThis.fetch = network.fetch;
    const create = async (file?: string) => {
      const identity = { store: "st-demo", project: "", role: "assistant" as const, assistant: "A-1" };
      const client = new CorralClient({ corral: "", root: work, cwd: work, identity });
      host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
        capabilities: { catalog: 8, digest: "b".repeat(64), preset: "standard", enabled: ["ask_user"] },
        ui: "plain", runtimeDir: work, client, events: new EventLog(), network,
        modules: { ...modules, role: assistantRole(PACKAGE_ROOT) },
        sessionManager: file ? sdk.SessionManager.open(file) : sdk.SessionManager.create(work, join(work, "sessions")) });
      await host.session.bindExtensions({ mode: "tui", uiContext: ui });
      await host.startLocal();
      return host;
    };
    let live = await create();
    const file = live.session.sessionFile!;
    const unrelated = join(work, "research.txt"); writeFileSync(unrelated, "independent research completed");
    transport.script([
      { toolCalls: [{ callId: "clarify", name: "ask_user", arguments: { nonBlocking: true,
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
    expect(statuses.get("corral-owner-questions")).toContain("1 owner question");
    expect(widget?.render(100).join("\n")).toContain("/questions");
    const q = live.session.sessionManager.getEntries().findLast(e => e.type === "custom" && e.customType === QUESTION_ENTRY);
    const id = q?.type === "custom" ? (q.data as { id: string }).id : "missing";
    const userAnswers = () => live.session.messages.flatMap(m => m.role !== "user" ? [] :
      [typeof m.content === "string" ? m.content : m.content.map(p => p.type === "text" ? p.text : "").join("")])
      .filter(message => message.startsWith(answerMarker(id)));
    expect(userAnswers()).toEqual([]);
    const promptEvents = () => live.liveEvents.since(0, EVENT_PAGE_BYTES).events
      .filter(e => e.kind === "owner_prompt")
      .map(({ question, status, prompts }) => ({ question, status, prompts }));
    const prompt = { question: "Which scope?", options: ["Small (recommended)", "Large"], freeText: true };
    const change = (status: string) => ({ question: id, status, prompts: [prompt] });
    expect(promptEvents()).toEqual([change("pending")]);
    await live.close();
    live = await create(file);
    expect(statuses.get("corral-owner-questions")).toContain("1 owner question");
    expect(widget?.render(100).join("\n")).toContain(id);
    expect(promptEvents()).toEqual([change("pending")]);
    transport.script([
      { toolCalls: [{ callId: "dependent", name: "read", arguments: { path: unrelated } }] },
      { text: "The owner selected the small fix; dependent work resumed." },
    ]);
    await live.session.prompt(`/questions ${id}`, { source: "interactive" });
    await live.session.waitForIdle(); await live.drained();
    const text = `${answerMarker(id)}\nWhich scope?: Small (recommended)\nOnly the small fix`;
    expect(userAnswers()).toEqual([text]);
    expect(promptEvents()).toEqual([change("pending"), change("answered")]);
    expect(live.session.messages.some(m => m.role === "toolResult" && m.toolCallId.startsWith("dependent"))).toBeTrue();
    await live.session.prompt(`/questions ${id}`, { source: "interactive" });
    expect(selections).toEqual(["Which scope?"]);
    await live.close(); live = await create(file);
    expect(userAnswers()).toEqual([text]);
    expect(statuses.get("corral-owner-questions")).toBeUndefined();
    expect(notices.some(n => n.includes("not delivered"))).toBeFalse();
  } finally {
    await host?.close(); globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv); assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 30_000);
