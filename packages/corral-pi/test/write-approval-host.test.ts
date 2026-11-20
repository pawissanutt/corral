import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";

// Prove the approval is enforced by the real Pi host even with ask_user off.
test("assistant writes outside its directory only on an exact owner-approved retry", async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-write-approval-"));
  const originalFetch = globalThis.fetch, originalEnv = { ...process.env };
  let host: HostSession | undefined;
  try {
    const cwd = join(work, "checkout");
    mkdirSync(cwd);
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant");
    const identity = { store: "st-demo", project: "", role: "assistant" as const, assistant: "A-1" };
    const transport = fakeCodexTransport(), network = guardedFetch(transport.send);
    globalThis.fetch = network.fetch;
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
      capabilities: { catalog: 8, digest: "a".repeat(64), preset: "strict", enabled: [] },
      ui: "plain", client: new CorralClient({ corral: "", root: work, cwd, identity }), events: new EventLog(), network,
      modules: { ...modules, role: assistantRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
    await host.session.bindExtensions({});
    await host.startLocal();
    const external = join(work, "external.txt"), protectedPath = join(work, ".corral-next", "state.txt");
    const write = (callId: string, path: string, content: string) => ({
      toolCalls: [{ callId, name: "write", arguments: { path, content } }],
    });
    transport.script([write("inside", "local.txt", "local"), write("external", external, "original"),
      write("protected", protectedPath, "not allowed"), { text: "Waiting." }]);
    await host.session.prompt("Make these fixture changes", { source: "extension" });
    await host.drained();
    expect(readFileSync(join(cwd, "local.txt"), "utf8")).toBe("local");
    expect(existsSync(external)).toBeFalse();
    expect(existsSync(protectedPath)).toBeFalse();
    const [question] = host.pendingOwnerQuestions();
    expect(host.pendingOwnerQuestions()).toHaveLength(1);
    expect(question?.question).toContain(external);
    transport.script([write("approved", external, "original"), write("repeat", external, "original"), { text: "Done." }]);
    await host.answerQuestion({ question: question!.id, option: "allow" }, async () => {});
    await host.session.waitForIdle();
    await host.drained();
    expect(readFileSync(external, "utf8")).toBe("original");
    const again = host.pendingOwnerQuestions();
    expect(again).toHaveLength(1);
    expect(again[0]?.id).not.toBe(question?.id);
  } finally {
    await host?.close(); globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv); assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 30_000);
