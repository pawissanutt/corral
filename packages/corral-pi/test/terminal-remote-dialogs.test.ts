import { expect, test } from "bun:test";
import { join } from "node:path";
import { startBridge } from "../src/bridge/server.ts";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { askDialog } from "../src/host/ask-user.ts";
import { questionEntries, questionStatus, type SessionCard } from "../src/host/remote/cards.ts";
import { RemoteDialogs, type DialogResolution } from "../src/host/remote/dialogs.ts";
import { installRemote } from "../src/host/remote/index.ts";
import type { SessionState } from "../src/host/remote/state.ts";
import { RemoteOwner } from "../src/host/remote-owner.ts";
import { SessionRecord } from "../src/host/session-record.ts";
import { PendingQuestions } from "../src/host/pending-questions.ts";
import { connect, testHandlers, type Reply } from "./support/bridge-client.ts";
import { codeHost } from "./support/code-host.ts";
import { nextEvent } from "./support/wait.ts";

interface TerminalFixture {
  dialogs: RemoteDialogs;
  request(command: string, args?: Record<string, unknown>): Promise<Reply>;
  nextState(): Promise<SessionState | undefined>;
}

async function withTerminal(run: (fixture: TerminalFixture) => Promise<void>): Promise<void> {
  const fx = await codeHost("assistant", { enabled: [] });
  try {
    const { host, events } = fx, id = "A-terminal", dir = join(fx.cwd, "socket");
    const owner = new RemoteOwner(host, events, new SessionRecord(dir, id, "assistant", fx.cwd), host.session.sessionFile!, dir);
    host.markAttached("interactive");
    installRemote(host, owner, dir);
    const bridge = await startBridge({ path: join(dir, "host.sock"), sessionId: id, events });
    try {
      bridge.attach(testHandlers(), host.liveEvents, host.remoteRegistry);
      const socket = await connect(bridge.path);
      try {
        const request = (command: string, args: Record<string, unknown> = {}) => socket.request({ version: 2, id: command, command, args });
        await run({ dialogs: host.dialogs, request, async nextState() {
          const event = await nextEvent(host.liveEvents.events, e => e.kind === "live_state", 2000);
          const reply = await request("live_since", { seq: event.seq - 1 });
          expect(reply.ok).toBeTrue();
          return reply.result?.state as SessionState | undefined;
        } });
      } finally { socket.close(); }
    } finally { await bridge.close(); }
  } finally { await fx.close(); }
}

test("a dialog opened in the terminal is listed as a card and a remote answer resolves it", async () => {
  await withTerminal(async ({ dialogs, request, nextState }) => {
    const changed = nextState();
    const prompt = dialogs.openPrompt({ kind: "confirm", title: "Use the project setting?" });
    const page = await request("cards");
    expect(page.ok).toBeTrue();
    expect(page.result?.cards).toMatchObject([{ id: prompt.id, kind: "confirm" }]);
    expect(await changed).toMatchObject({ mode: "interactive", cards: [{ id: prompt.id, kind: "confirm" }] });
    const options = (page.result?.cards as SessionCard[])[0]!.questions[0]!.options;
    const answer = { card: prompt.id, answers: [{ options: [options[0]!.id] }], device: "phone-2" };
    expect((await request("card_answer", { ...answer, device: "é".repeat(33) })).error?.code).toBe("bad_input");
    expect(prompt.settled).toBeFalse();
    expect((await request("card_answer", answer)).result).toEqual({ status: "answered" });
    expect(await prompt.value).toBe("Yes");
    expect(await prompt.result).toEqual({ kind: "answered", answers: answer.answers, by: { source: "remote", device: "phone-2" } });
    expect((await request("cards")).result).toEqual({ cards: [] });
  });
});

test("the second answer to one dialog is refused whichever side sent the first", async () => {
  await withTerminal(async ({ dialogs, request }) => {
    const terminalFirst = dialogs.openPrompt({ kind: "select", title: "Which scope?", options: ["Project", "Branch"] });
    expect(terminalFirst.answer("Project")).toBe("answered");
    const later = await request("card_answer", { card: terminalFirst.id, answers: [{ options: ["Branch"] }] });
    expect(later.error?.code).toBe("invalid_card_answer");
    expect(await terminalFirst.result).toMatchObject({ kind: "answered", by: { source: "terminal" } });
    expect(await terminalFirst.value).toBe("Project");

    const remoteFirst = dialogs.openPrompt({ kind: "input", title: "Name" });
    const first = await request("card_answer", { card: remoteFirst.id, answers: [{ text: "phone answer" }] });
    expect(first.result).toEqual({ status: "answered" });
    let refusal: unknown;
    try { remoteFirst.answer("terminal answer"); } catch (error) { refusal = error; }
    expect(refusal).toMatchObject({ code: "invalid_card_answer" });
    expect(await remoteFirst.value).toBe("phone answer");
  });
});

test("a dialog cancelled in the terminal leaves no card", async () => {
  await withTerminal(async ({ dialogs, request }) => {
    const prompt = dialogs.openPrompt({ kind: "confirm", title: "Continue?" });
    expect(prompt.cancel()).toBe("dismissed");
    expect(await prompt.result).toEqual({ kind: "cancelled", reason: "dismissed", by: { source: "terminal" } });
    expect(await prompt.value).toBeUndefined();
    expect((await request("cards")).result).toEqual({ cards: [] });
    expect((await request("card_answer", { card: prompt.id, answers: [{ options: ["yes"] }] })).result).toEqual({ status: "missing" });
  });
});

test("a terminal watcher and the app answer the existing durable question under the same ID", async () => {
  let deliveries = 0;
  const permit = Promise.withResolvers<void>();
  const pending = new PendingQuestions({ session: () => "durable", append() {}, entries: () => [], changed() {},
    async deliver() { deliveries++; return true; } });
  const dialogs = new RemoteDialogs(() => {}, {
    entries: () => questionEntries(pending),
    absent: id => questionStatus(pending.status(id)), shared: () => true,
  });
  try {
    const question = pending.ask("ask", [{ question: "Which?", options: ["first", "second"] }]);
    const observed = Promise.withResolvers<DialogResolution>();
    const terminal = dialogs.watch(card => card.id === question.id, observed.resolve)!;
    expect(terminal.id).toBe(question.id);
    expect(dialogs.entries().map(entry => entry.card.id)).toEqual([question.id]);
    const answers = [{ options: ["second"] }];
    const remote = dialogs.resolve(question.id, answers, false, { source: "remote", device: "desktop" }, () => permit.promise);
    await expect(terminal.answer([{ options: ["first"] }])).rejects.toMatchObject({ code: "invalid_card_answer" });
    permit.resolve();
    expect(await remote).toBe("answered");
    expect(await observed.promise).toEqual({ kind: "answered", answers, by: { source: "remote", device: "desktop" } });
    expect(deliveries).toBe(1);
    expect(dialogs.entries()).toEqual([]);
  } finally { permit.resolve(); dialogs.close(); pending.close(); }
});

test("a dismissed shared card rejects the other surface's late answer", async () => {
  const card: SessionCard = { id: "plan-1", kind: "board_plan", title: "Plan", body_more: false, opened_at: "2026-01-01T00:00:00Z", questions: [] };
  let proposed = true;
  const dialogs = new RemoteDialogs(() => {}, {
    entries: () => proposed ? [{ card, answer: () => { proposed = false; return "dismissed"; } }] : [],
    absent: () => "missing", shared: () => true,
  });
  try {
    const observed = Promise.withResolvers<DialogResolution>();
    const dock = dialogs.watch(candidate => candidate.id === card.id, observed.resolve)!;
    expect(await dock.answer([{ options: ["dismiss"] }])).toBe("dismissed");
    expect(await observed.promise).toEqual({ kind: "cancelled", reason: "dismissed", by: { source: "terminal" } });
    await expect(dialogs.resolve(card.id, [{ options: ["file"] }], false, { source: "remote" }))
      .rejects.toMatchObject({ code: "invalid_card_answer" });
  } finally { dialogs.close(); }
});

test("remote answers finish the synchronous terminal question batch", async () => {
  const dialogs = new RemoteDialogs(() => {});
  const done = Promise.withResolvers<unknown>();
  const terminal: { component?: { handleInput(data: string): void } } = {};
  const ui = { custom(factory: (...args: unknown[]) => typeof terminal.component) {
    terminal.component = factory({ requestRender() {} }, {}, {}, done.resolve);
    return done.promise;
  } } as unknown as ExtensionUIContext;
  try {
    const batch = askDialog(ui, [
      { question: "Which lanes?", options: [{ label: "quick" }, { label: "full" }], multiSelect: true },
      { question: "Why?", options: [{ label: "one" }, { label: "two" }] },
    ], undefined, undefined, dialogs);
    const card = dialogs.entries()[0]!.card;
    await expect(dialogs.resolve(card.id, [{ options: ["quick", "quick"] }], false, { source: "remote" }))
      .rejects.toMatchObject({ code: "invalid_card_answer" });
    expect(dialogs.entries()).toHaveLength(1);
    expect(await dialogs.resolve(card.id, [
      { options: ["quick", "full"] }, { options: [], text: "Both paths changed" },
    ], false, { source: "remote" })).toBe("answered");
    terminal.component?.handleInput("\r");
    expect(await batch).toEqual({ submitted: true, answers: [
      { selected: ["quick", "full"], text: "" }, { selected: [], text: "Both paths changed" },
    ] });
    expect(dialogs.entries()).toEqual([]);
  } finally { dialogs.close(); }
});
