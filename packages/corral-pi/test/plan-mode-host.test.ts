// Plan mode on the production HostSession with a deterministic provider: the read-only guard, the review card
// and a clean-context execution, through both ways the context can be reset.
import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import type { CapabilityGuidance } from "../src/capabilities/guidance.ts";
import { loadModules } from "../src/host/modules.ts";
import { PLAN_NOTE } from "../src/host/plan-mode.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";

async function until(done: () => boolean, what: string): Promise<void> {
  await within((async () => { while (!done()) await new Promise(resolve => setTimeout(resolve, 5)); })(), 30_000, what);
}

const VARIANTS: readonly { name: string; compacts: boolean; guidance?: CapabilityGuidance }[] = [
  // The SDK refuses to compact a small session, so the reset entry is appended directly.
  { name: "a session too small to compact", compacts: false },
  // With almost nothing kept recent the SDK compacts, and the host supplies the reset as that compaction.
  { name: "a compactable session", compacts: true, guidance: { settings: { compaction: { keep_recent_tokens: 1 } }, projectText: false } },
];

for (const variant of VARIANTS) {
  test(`plan mode refuses worktree writes, then runs the approved plan from a clean context in ${variant.name}`, async () => {
    const assets = buildFixtureInstall();
    const work = mkdtempSync(join(tmpdir(), "plan-mode-"));
    const cwd = join(work, "ws");
    mkdirSync(cwd, { recursive: true });
    const originalFetch = globalThis.fetch;
    const originalEnv = { ...process.env };
    let host: HostSession | undefined;
    try {
      restrictEnvironment(process.env);
      const sdk = await import("@earendil-works/pi-coding-agent");
      const modules = await loadModules(assets.dir, "assistant");
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const identity: HostIdentity = { store: "st-demo", project: "", role: "assistant", assistant: "A-1" };
      host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
        capabilities: { catalog: 8, digest: "b".repeat(64), preset: "standard", enabled: ["ask_user", "compaction"] },
        ...(variant.guidance ? { guidance: variant.guidance } : {}),
        ui: "plain", runtimeDir: work, client: new CorralClient({ corral: "", root: work, cwd, identity }), events: new EventLog(), network,
        modules: { ...modules, role: assistantRole(PACKAGE_ROOT) },
        sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
      await host.session.bindExtensions({ mode: "tui", uiContext: { setStatus: () => {} } as unknown as ExtensionUIContext });
      await host.startLocal();
      const live = host, plan = live.planMode;
      if (!plan) throw new Error("the assistant role has no plan mode");
      const write = (callId: string, path: string, content: string) => ({ toolCalls: [{ callId, name: "write", arguments: { path, content } }] });
      const texts = () => live.session.messages.map(message => {
        const content = "content" in message ? message.content : "summary" in message ? message.summary : "";
        return typeof content === "string" ? content : content.map(part => part.type === "text" ? part.text : "").join("");
      });
      const entries = () => live.session.sessionManager.getEntries() as { type: string; customType?: string; details?: { kind?: string } }[];
      const outside = join(cwd, "a.txt");

      // Given plan mode and an agent that tries to implement before it plans:
      await plan.enter();
      fixtureTransport().script([write("early", outside, "x"), write("plan", plan.path, "# Add the flag\n\n1. Parse --version.\n"),
        { toolCalls: [{ callId: "submit", name: "plan_submit", arguments: {} }] }, { text: "Submitted." }]);
      await live.session.prompt("Plan a --version flag", { source: "extension" });
      await live.drained();
      // Then only the plan document was written, and the submission is the one open card.
      expect(existsSync(outside)).toBeFalse();
      expect(texts().some(text => text.includes("Plan mode: the working tree is read-only"))).toBeTrue();
      expect(entries().filter(entry => entry.type === "custom_message" && entry.customType === PLAN_NOTE).map(entry => entry.details?.kind))
        .toEqual(["contract"]);
      expect(plan.brief()).toEqual({ phase: "review", title: "Add the flag", revision: 1 });
      const card = live.dialogs.entries().find(entry => entry.card.kind === "plan_review");
      expect(card?.card).toMatchObject({ id: plan.review, title: "Add the flag", body: "# Add the flag\n\n1. Parse --version.\n" });

      // When the owner executes it with a clean context:
      let compacted: boolean | undefined;
      const compact = live.session.compact.bind(live.session);
      live.session.compact = instructions => compact(instructions).then(result => { compacted = true; return result; },
        error => { compacted = false; throw error; });
      fixtureTransport().script([write("late", outside, "x"), { text: "Done." }]);
      expect(await card?.answer([{ options: ["execute_clean"] }], false, async () => {})).toBe("started");
      await until(() => plan.phase === "off" && texts().at(-1) === "Done.", "the approved plan to run");
      await live.drained();
      expect(compacted).toBe(variant.compacts);
      // Then the model's context holds the reset summary and the plan, none of the planning, and writes work again.
      expect(texts().some(text => text.includes("Plan a --version flag") || text === "Submitted.")).toBeFalse();
      const kickoff = texts().find(text => text.startsWith("[corral plan approved]"));
      expect(kickoff).toContain("# Add the flag");
      expect(kickoff).toContain("cleared at the owner's request");
      expect(entries().filter(entry => entry.type === "compaction")).toHaveLength(1);
      expect(readFileSync(join(work, "plans", "1-add-the-flag.md"), "utf8")).toBe("# Add the flag\n\n1. Parse --version.\n");
      expect(existsSync(outside)).toBeTrue();
      expect(live.dialogs.entries().some(entry => entry.card.kind === "plan_review")).toBeFalse();
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
