import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import { startBridge } from "../src/bridge/server.ts";
import { FleetRuntime } from "../src/extensions/fleet/runtime.ts";
import type { ChildHooks } from "../src/extensions/fleet/child.ts";
import { attachHeadless } from "../src/host/headless.ts";
import { PendingQuestions } from "../src/host/pending-questions.ts";
import { RemoteBuild } from "../src/host/remote/build.ts";
import { questionEntries, questionStatus, registerCards, type CardAnswer, type SessionCard } from "../src/host/remote/cards.ts";
import { RemoteDialogs } from "../src/host/remote/dialogs.ts";
import { installRemote } from "../src/host/remote/index.ts";
import { RemoteRegistry } from "../src/host/remote/registry.ts";
import { RemoteOwner } from "../src/host/remote-owner.ts";
import { SessionRecord } from "../src/host/session-record.ts";
import type { ChildMark } from "../src/host/transcript-marks.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";
import { codeHost } from "./support/code-host.ts";
import { within } from "./support/wait.ts";

function cards(dialogs: RemoteDialogs) {
  const registry = new RemoteRegistry();
  registerCards(registry, { dialogs, ready: () => true, beforeQuestion: async () => {} });
  return {
    list: async (after?: string) => await registry.get("cards")!({ after }) as { cards: SessionCard[]; next?: string },
    answer: async (card: string, answers: CardAnswer[] = [], dismiss = false) =>
      await registry.get("card_answer")!({ card, answers, dismiss }) as { status: string },
  };
}

test("a deferred question, a select and a child question are each a card and each is answered once", async () => {
  const fx = await codeHost("assistant", { enabled: [] });
  const deliveries: string[] = [], marks: ChildMark[] = [];
  const pending = new PendingQuestions({ session: () => "parent", append() {}, entries: () => [], changed() {},
    async deliver(text) { deliveries.push(text); return true; } });
  const started = Promise.withResolvers<ChildHooks>();
  const fleet = new FleetRuntime({ sdk: {} as typeof Sdk, models: { getModels: () => [] } as unknown as Sdk.ModelRuntime,
    approved: ["fixture"], parent: { provider: "fixture", model: "fixture-model" },
    context: { dir: join(fx.cwd, "fleet"), session: { worktree: fx.cwd, name: "fixture" }, settings: { maxParallel: 1, profiles: [] } },
    agentDir: join(fx.cwd, "agent"), localTools: [], deliverToParent() {}, changed() {},
    runner: { run(_spec, _model, hooks, signal) {
      const done = Promise.withResolvers<{ report: string }>();
      signal.addEventListener("abort", () => done.resolve({ report: "cancelled" }), { once: true });
      started.resolve(hooks);
      return { done: done.promise, usage: () => ({ input: 0, output: 0, turns: 0 }), async steer() {}, async setModel() { return "applied"; } };
    } },
  });
  const build = new RemoteBuild(undefined, { fleet: () => fleet, children: () => fleet.snapshot().children,
    ready: () => true, headless: () => true, working: () => false, changed() {}, post: async () => {}, error() {},
    mark(mark) { marks.push(mark); },
  });
  const dialogs = new RemoteDialogs(() => {}, {
    entries: () => [...questionEntries(pending), ...build.entries()],
    absent: id => questionStatus(pending.status(id)), shared: () => false,
  });
  const selected = dialogs.context(fx.host.session.extensionRunner.getUIContext(), () => {});
  void fleet.start("test", [{ id: "slice", title: "Test slice", brief: "Ask the owner", owns: [] }]);
  try {
    const id = pending.ask("ask", [{ question: "First?", options: ["one", "two"] }, { question: "Second?", options: ["three", "four"] }]).id;
    const selection = selected.select("Pick", ["blue", "red"]);
    const hooks = await within(started.promise, 2000, "child starts");
    const childQuestion = { question: "Which path?", options: [{ label: "short" }, { label: "long" }], askedAt: Date.now() };
    const answer = hooks.ask(childQuestion);
    void answer.catch(() => {});
    const socket = cards(dialogs);
    const { cards: shown } = await socket.list();
    expect(shown.map(card => card.kind).sort()).toEqual(["child_question", "question", "select"]);
    const deferred = shown.find(card => card.id === id)!;
    const select = shown.find(card => card.kind === "select")!, child = shown.find(card => card.kind === "child_question")!;
    expect(deferred.questions).toHaveLength(2);
    await expect(socket.answer(id, [{ options: ["one"] }])).rejects.toMatchObject({ code: "invalid_card_answer" });
    expect(await socket.answer(id, [], true)).toEqual({ status: "dismissed" });
    const batch = [{ options: ["one"] }, { options: [], text: "a different answer" }];
    expect(await Promise.all([socket.answer(id, batch), socket.answer(id, batch)])).toEqual([{ status: "answered" }, { status: "answered" }]);
    expect(deliveries).toHaveLength(1);
    expect(await socket.answer(select.id, [{ options: ["red"] }])).toEqual({ status: "answered" });
    expect(await socket.answer(select.id, [{ options: ["blue"] }])).toEqual({ status: "missing" });
    expect(await selection).toBe("red");
    expect(await socket.answer(child.id, [{ options: ["short"] }])).toEqual({ status: "delivered" });
    expect(await answer).toBe("short");
    expect(await socket.answer(child.id, [{ options: ["long"] }])).toEqual({ status: "missing" });
    expect(marks.map(mark => mark.act)).toEqual(["asked", "answered"]);
    expect((await socket.list()).cards).toEqual([]);
    const later = hooks.ask(childQuestion);
    void later.catch(() => {});
    const next = (await socket.list()).cards[0]!;
    expect(next.id).not.toBe(child.id);
    expect(await socket.answer(child.id, [{ options: ["long"] }])).toEqual({ status: "missing" });
    expect(await socket.answer(next.id, [{ options: [], text: "the third path" }])).toEqual({ status: "delivered" });
    expect(await later).toBe("the third path");
  } finally { pending.close(); dialogs.close(); await fleet.close(); await fx.close(); }
});

test("a dismissed confirm answers no", async () => {
  const fx = await codeHost("assistant", { enabled: [] });
  const dialogs = new RemoteDialogs(() => {});
  try {
    const ui = dialogs.context(fx.host.session.extensionRunner.getUIContext(), () => {});
    const socket = cards(dialogs);
    const confirmed = ui.confirm("Continue?", "Choose whether to continue.");
    const card = (await socket.list()).cards[0]!;
    expect(card.kind).toBe("confirm");
    expect(await socket.answer(card.id, [], true)).toEqual({ status: "dismissed" });
    expect(await confirmed).toBeFalse();
    expect(await socket.answer(card.id, [{ options: ["yes"] }])).toEqual({ status: "missing" });

    const abort = new AbortController();
    const input = ui.input("Name?", "type here", { signal: abort.signal });
    const waiting = (await socket.list()).cards[0]!;
    expect(waiting.kind).toBe("input");
    abort.abort();
    expect(await input).toBeUndefined();
    expect(await socket.answer(waiting.id, [{ options: [], text: "late" }])).toEqual({ status: "expired" });
  } finally { dialogs.close(); await fx.close(); }
});

test("large Unicode dialog cards page without losing answers or exceeding either reply bound", async () => {
  const fx = await codeHost("assistant", { enabled: [] });
  const dialogs = new RemoteDialogs(() => {});
  try {
    const ui = dialogs.context(fx.host.session.extensionRunner.getUIContext(), () => {});
    const socket = cards(dialogs);
    const body = "\u0000界🪶\n".repeat(2000);
    const pending = Array.from({ length: 7 }, (_, i) => ui.confirm(`Confirm ${i}`, body));
    const ids = new Set<string>();
    let after: string | undefined;
    do {
      const page = await socket.list(after);
      expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(48 * 1024);
      for (const card of page.cards) {
        expect(ids.has(card.id)).toBeFalse();
        ids.add(card.id);
        expect(Buffer.byteLength(JSON.stringify(card))).toBeLessThanOrEqual(12 * 1024);
        expect(Buffer.byteLength(card.body!)).toBeLessThanOrEqual(8 * 1024);
        expect(body.startsWith(card.body!)).toBeTrue();
        expect(card.body_more).toBeTrue();
        expect(await socket.answer(card.id, [{ options: ["yes"] }])).toEqual({ status: "answered" });
      }
      after = page.next;
    } while (after !== undefined);
    expect(ids.size).toBe(pending.length);
    expect((await Promise.all(pending)).every(Boolean)).toBeTrue();
  } finally { dialogs.close(); await fx.close(); }
});

test("ask_user stays deferred in a headless session that has a remote dialog context", async () => {
  const fx = await codeHost("assistant", { enabled: ["ask_user"] });
  try {
    await attachHeadless(fx.host, fx.events, () => {});
    expect(fx.host.session.extensionRunner.hasUI()).toBeTrue();
    await within(fx.turn([{ toolCalls: [{ callId: "owner", name: "ask_user", arguments: { questions: [
      { question: "Decision 1?", options: ["a", "b"] }, { question: "Decision 2?", options: ["c", "d"] },
    ] } }] }, { text: "Waiting for the owner." }]), 3000, "the deferred turn settles");
    const cards = fx.host.dialogs.entries().map(entry => entry.card);
    expect(cards.map(card => card.kind)).toEqual(["question"]);
    expect(cards[0]?.questions).toHaveLength(2);
    expect(fx.result("owner").details).toMatchObject({ status: "pending", id: cards[0]?.id });
    const journal = fx.host.session.sessionManager.getEntries();
    expect(journal.some(entry => entry.type === "custom" && entry.customType === "corral_owner_question")).toBeTrue();
  } finally { await fx.close(); }
});

test("a startup dialog can be answered on the bound socket before extension binding finishes", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-start-dialog-"));
  const entered = Promise.withResolvers<void>();
  let selected: string | undefined;
  const fx = await codeHost("assistant", { enabled: [], extend: pi => pi.on("session_start", async (_event, ctx) => {
    if (!ctx.hasUI) return;
    const answer = ctx.ui.select("Start?", ["Continue"]);
    entered.resolve();
    selected = await answer;
  }) });
  const { host, events } = fx, file = host.session.sessionFile!;
  const record = new SessionRecord(dir, "A-start", "assistant", fx.cwd);
  const owner = new RemoteOwner(host, events, record, file, dir);
  const bridge = await startBridge({ path: join(dir, "host.sock"), sessionId: "A-start", events });
  const client = await connect(bridge.path);
  const call = (command: string, args = {}) => client.request({ version: 2, id: command, command, args });
  try {
    expect((await call("submit_input", { origin: "owner", text: "Too early" })).error?.code).toBe("starting");
    const bound = attachHeadless(host, events, () => {}, () => {
      installRemote(host, owner, dir);
      bridge.attach(testHandlers({ hello: () => ({ session_id: "A-start", session_file: file }) }), host.liveEvents, host.remoteRegistry);
    });
    await within(entered.promise, 2000, "startup dialog");
    expect((await call("hello")).result).toMatchObject({ session_id: "A-start", session_file: file });
    const initial = (await call("cards")).result as unknown as { cards: SessionCard[] };
    expect(initial.cards.map(card => card.kind)).toEqual(["select"]);
    expect((await call("card_answer", { card: initial.cards[0]!.id, answers: [{ options: ["Continue"] }] })).result).toEqual({ status: "answered" });
    await within(bound, 2000, "extension binding finishes");
    expect(selected).toBe("Continue");
  } finally { client.close(); await bridge.close(); await fx.close(); rmSync(dir, { recursive: true, force: true }); }
});
