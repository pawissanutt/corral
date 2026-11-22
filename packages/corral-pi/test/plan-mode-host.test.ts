// Plan mode on the production HostSession with a deterministic provider: the read-only guard, the review card
// and a clean-context execution, through both ways the context can be reset.
import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import type { CapabilityGuidance } from "../src/capabilities/guidance.ts";
import { loadModules } from "../src/host/modules.ts";
import { PLAN_NOTE, type PlanMode } from "../src/host/plan-mode.ts";
import { RemoteOwner } from "../src/host/remote-owner.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { SessionRecord } from "../src/host/session-record.ts";
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

/** An assistant host in a temporary workspace with its production pause and admission order, already in plan mode. */
async function planning(guidance: CapabilityGuidance | undefined,
  run: (host: HostSession, plan: PlanMode, cwd: string, owner: RemoteOwner) => Promise<void>): Promise<void> {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "plan-mode-"));
  const cwd = join(work, "ws");
  mkdirSync(cwd, { recursive: true });
  const originalFetch = globalThis.fetch;
  const originalEnv = { ...process.env };
  let host: HostSession | undefined;
  let owner: RemoteOwner | undefined;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity: HostIdentity = { store: "st-demo", project: "", role: "assistant", assistant: "A-1" };
    const events = new EventLog(), record = new SessionRecord(work, "A-1", "assistant", cwd, true);
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
      capabilities: { catalog: 8, digest: "b".repeat(64), preset: "standard", enabled: ["ask_user", "compaction"] },
      ...(guidance ? { guidance } : {}), paused: () => record.paused, startInput: input => owner?.start(input),
      ui: "plain", runtimeDir: work, client: new CorralClient({ corral: "", root: work, cwd, identity }), events, network,
      modules: { ...modules, role: assistantRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
    await host.session.bindExtensions({ mode: "tui", uiContext: { setStatus: () => {} } as unknown as ExtensionUIContext });
    await host.startLocal();
    owner = new RemoteOwner(host, events, record, host.session.sessionFile ?? "", work);
    if (!host.planMode) throw new Error("the assistant role has no plan mode");
    await host.planMode.enter();
    await run(host, host.planMode, cwd, owner);
  } finally {
    await host?.close();
    globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv);
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}

const PLAN = "# Add the flag\n\n1. Parse --version.\n";
const SUBMIT = { toolCalls: [{ callId: "submit", name: "plan_submit", arguments: {} }] };
const write = (callId: string, path: string, content: string) => ({ toolCalls: [{ callId, name: "write", arguments: { path, content } }] });
const answerCard = (host: HostSession) => {
  const card = host.dialogs.entries().find(entry => entry.card.kind === "plan_review");
  if (!card) throw new Error("no plan review card");
  return card;
};

const VARIANTS: readonly { name: string; compacts: boolean; guidance?: CapabilityGuidance }[] = [
  // The SDK refuses to compact a small session, so the reset entry is appended directly.
  { name: "a session too small to compact", compacts: false },
  // With almost nothing kept recent the SDK compacts, and the host supplies the reset as that compaction.
  { name: "a compactable session", compacts: true, guidance: { settings: { compaction: { keep_recent_tokens: 1 } }, projectText: false } },
];

for (const variant of VARIANTS) {
  test(`plan mode refuses worktree writes, then runs the reviewed plan from a clean context in ${variant.name}`,
    () => planning(variant.guidance, async (live, plan, cwd) => {
      const texts = () => live.session.messages.map(message => {
        const content = "content" in message ? message.content : "summary" in message ? message.summary : "";
        return typeof content === "string" ? content : content.map(part => part.type === "text" ? part.text : "").join("");
      });
      const entries = () => live.session.sessionManager.getEntries() as
        { type: string; customType?: string; summary?: string; details?: { kinds?: string[] } }[];
      const outside = join(cwd, "a.txt");

      // Given an agent that tries to implement before it plans:
      fixtureTransport().script([write("early", outside, "x"), write("plan", plan.path, PLAN), SUBMIT, { text: "Submitted." }]);
      await live.session.prompt("Plan a --version flag", { source: "extension" });
      await live.drained();
      // Then only the plan document was written, and the submission is the one open card.
      expect(existsSync(outside)).toBeFalse();
      expect(texts().some(text => text.includes("Plan mode: the working tree is read-only"))).toBeTrue();
      expect(entries().filter(entry => entry.type === "custom_message" && entry.customType === PLAN_NOTE).map(entry => entry.details?.kinds))
        .toEqual([["contract"]]);
      expect(plan.brief()).toEqual({ phase: "review", title: "Add the flag", revision: 1 });

      // Given the working file changes after the submission, as a shell command could change it:
      writeFileSync(plan.path, "tampered\n");
      // Then the review still shows the submitted text, and no other card can read it.
      const card = answerCard(live);
      expect(card.card).toMatchObject({ id: plan.review, title: "Add the flag", body: PLAN });
      expect(plan.doc(card.card.id)).toMatchObject({ body: PLAN, revision: 1 });
      expect(() => plan.doc("plan-1-other")).toThrow("no plan is under review with this card");

      // When the owner executes it with a clean context:
      let compacted: boolean | undefined;
      const compact = live.session.compact.bind(live.session);
      live.session.compact = instructions => compact(instructions).then(result => { compacted = true; return result; },
        error => { compacted = false; throw error; });
      fixtureTransport().script([write("late", outside, "x"), { text: "Done." }]);
      expect(await card.answer([{ options: ["execute_clean"] }], false, async () => {})).toBe("started");
      await until(() => plan.phase === "off" && texts().at(-1) === "Done.", "the approved plan to run");
      await live.drained();
      expect(compacted).toBe(variant.compacts);
      // Then the reset itself holds the reviewed plan, none of the planning survives, and writes work again.
      const resets = entries().filter(entry => entry.type === "compaction");
      expect(resets).toHaveLength(1);
      expect(resets[0]?.summary).toContain(PLAN.trimEnd());
      expect(texts().some(text => text.includes("Plan a --version flag") || text === "Submitted." || text.includes("tampered"))).toBeFalse();
      expect(texts().find(text => text.startsWith("[corral plan approved "))).toContain("Execute the plan in the summary above");
      expect(readFileSync(plan.approved?.path ?? "", "utf8")).toBe(PLAN);
      expect(existsSync(outside)).toBeTrue();
      expect(live.dialogs.entries().some(entry => entry.card.kind === "plan_review")).toBeFalse();
    }), 120_000);
}

test("a plan review is withdrawn only by an owner message the session takes", () => planning(undefined, async (live, plan, _cwd, owner) => {
  // Given a submitted plan:
  writeFileSync(plan.path, PLAN);
  fixtureTransport().script([SUBMIT, { text: "Submitted." }]);
  await live.session.prompt("Plan a --version flag", { source: "extension" });
  await live.drained();
  const review = plan.review;
  expect(review).toBeDefined();

  // A paused session would hold the plan instead of starting it, so the choice is refused.
  await owner.pause();
  expect(() => answerCard(live).answer([{ options: ["execute"] }], false, async () => {})).toThrow("The session is paused");
  expect(plan.review).toBe(review);
  await owner.resume();

  // Given an automated turn that is still running:
  fixtureTransport().script([{ text: "Noted." }]);
  fixtureTransport().holdNext();
  const held = fixtureTransport().whenHeld();
  void live.submitLocal({ text: "an automated note", origin: "auto", busyPolicy: "steer" });
  await held;
  // When the owner's reply cannot be taken, from the app or typed in the terminal:
  const refused = await live.submitLocal({ text: "Change step 1", origin: "owner", busyPolicy: "prompt" });
  expect(refused.disposition).toBe("rejected");
  await expect(live.session.prompt("Change step 1", { source: "interactive" })).rejects.toThrow("already processing");
  fixtureTransport().release();
  await until(() => !live.session.isStreaming, "the held turn to end");
  await live.drained();
  // Then the same review is still open, and the same words from anything but the owner leave it open too.
  expect(plan.review).toBe(review);
  fixtureTransport().script([{ text: "Automated." }]);
  await live.session.prompt("Change step 1", { source: "extension" });
  await live.drained();
  expect(plan.review).toBe(review);

  // When the session takes the owner's message, it is the feedback:
  fixtureTransport().script([{ text: "Revising." }]);
  await live.session.prompt("Change step 1", { source: "interactive" });
  await live.drained();
  expect(plan.phase).toBe("planning");
}), 120_000);

test("a pause that follows an approval cancels the plan's turn instead of missing it", () => planning(undefined, async (live, plan, _cwd, owner) => {
  writeFileSync(plan.path, PLAN);
  fixtureTransport().script([SUBMIT, { text: "Submitted." }]);
  await live.session.prompt("Plan a --version flag", { source: "extension" });
  await live.drained();
  // Given Pi is still admitting the approved plan's kickoff when the owner pauses:
  const admission = Promise.withResolvers<void>(), preflight = Promise.withResolvers<void>();
  const prompt = live.session.prompt.bind(live.session);
  live.session.prompt = async (text, options) => {
    preflight.resolve();
    await admission.promise;
    return prompt(text, options);
  };
  expect(answerCard(live).answer([{ options: ["execute"] }], false, async () => {})).toBe("started");
  await preflight.promise;
  const paused = owner.pause();
  admission.resolve();
  // Then the pause waits for that admission and cancels the turn it opened, rather than finding nothing to cancel.
  expect(await paused).toMatchObject({ cancelled: true, paused: true });
  await until(() => !live.session.isStreaming, "the cancelled turn to end");
  await live.drained();
  expect(plan.phase).toBe("off");
}), 120_000);

test("a compaction inside a planning run is followed by the contract before the run goes on",
  () => planning({ settings: { compaction: { keep_recent_tokens: 1 } }, projectText: false }, async (live, plan) => {
    // Given a planning turn whose first response reports a context past the model's window:
    fixtureTransport().script([{ ...write("plan", plan.path, PLAN), usage: { input: 50_000_000, output: 1 } },
      { text: "SUMMARY" }, { text: "Planned." }]);
    await live.session.prompt("Plan a --version flag", { source: "extension" });
    await live.drained();
    // Then Pi compacted before its next request, and the contract follows the summary inside the same run.
    const order = (live.session.sessionManager.getEntries() as { type: string; customType?: string; message?: { role?: string } }[])
      .flatMap(entry => entry.type === "compaction" ? ["compaction"]
        : entry.type === "custom_message" && entry.customType === PLAN_NOTE ? ["contract"]
        : entry.type === "message" && entry.message?.role === "assistant" ? ["assistant"] : []);
    expect(order).toEqual(["contract", "assistant", "compaction", "contract", "assistant"]);
  }), 120_000);
