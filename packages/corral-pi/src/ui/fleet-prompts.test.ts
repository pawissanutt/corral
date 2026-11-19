import { expect, test } from "bun:test";
import type { PresentedDialog } from "@earendil-works/pi-coding-agent";
import type { TUI } from "@earendil-works/pi-tui";
import type { ChildSnapshot, FleetControl } from "../build/types.ts";
import { RemoteBuild } from "../host/remote/build.ts";
import { RemoteDialogs } from "../host/remote/dialogs.ts";
import type { ChildMark } from "../host/transcript-marks.ts";
import { childQuestionCard } from "./build/build-controller.ts";
import { CHOICE_GUARD_MS } from "./build/choice-dock.ts";
import { childPrompts } from "./fleet-prompts.ts";
import { DialogAdoption, ownerDialogPresentation } from "./owner-dialog.ts";

const tui = { requestRender() {} } as unknown as TUI;
/** The registry's answers settle within a fixed number of microtask turns. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 50; turn++) await Promise.resolve();
}
/** Runs `keys` with the clock past the guard of every card opened before it, without waiting. */
function armed(keys: () => void): void {
  const real = Date.now;
  Date.now = () => real() + CHOICE_GUARD_MS;
  try { keys(); } finally { Date.now = real; }
}
const asking = (askedAt: number): ChildSnapshot => ({
  id: "scout", kind: "slice", call: "call-1", title: "scout slice", brief: "brief", owns: [], after: [], profile: "default", model: "anthropic/claude",
  state: "needs", createdAt: 0, edits: [], usage: { input: 0, output: 0, turns: 0 }, steers: [], pulse: [], history: "", messages: [],
  question: { question: "Which store?", askedAt, options: [{ label: "SQLite" }, { label: "Files" }] },
});

/**
 * A worker session attached to a terminal: the host lists each child's question as a card (`RemoteBuild` without
 * build controls), and Pi presents its select and input as the owner card at once, as InteractiveMode does.
 */
function workerSession() {
  let children: ChildSnapshot[] = [];
  const answers: string[] = [], marks: ChildMark[] = [], notices: { text: string; tone: string }[] = [], cards: PresentedDialog[] = [];
  const fleet: FleetControl = {
    snapshot: () => ({ children, mail: [], maxParallel: 3 }),
    steer: async () => true,
    cancel: () => true,
    setModel: () => {},
    answer: (id, text) => {
      answers.push(`${id} ${text}`);
      children = children.map((child): ChildSnapshot => child.id === id ? { ...child, state: "running", question: undefined } : child);
      return true;
    },
    models: () => [],
    profiles: () => [],
    message: async () => { throw new Error("Unused fixture messaging"); },
    roster: () => [],
    wait: async () => [],
    subscribe: () => () => {},
  };
  const build = new RemoteBuild(undefined, { fleet: () => fleet, children: () => children, ready: () => true, headless: () => false,
    working: () => false, changed() {}, mark: mark => { marks.push(mark); }, post: async () => {}, error() {} });
  const dialogs = new RemoteDialogs(() => {}, { entries: () => build.entries(), absent: () => "missing", shared: () => true });
  const adoption = new DialogAdoption();
  const presentation = ownerDialogPresentation({ rows: () => 24, chromeRows: () => 8, changed() {}, dialogs, adoption,
    notice: text => { notices.push({ text, tone: "text" }); } });
  const shown = (card: PresentedDialog | undefined) => { if (!card) throw new Error("the dialog hook declined"); cards.push(card); };
  const prompts = childPrompts({
    fleet,
    select: (title, options) => {
      const picked = Promise.withResolvers<string | undefined>();
      shown(presentation.select?.({ kind: "select", title, options, tui, toggleToolsExpanded() {} }, picked.resolve));
      return picked.promise;
    },
    input: title => {
      const typed = Promise.withResolvers<string | undefined>();
      shown(presentation.input?.({ title, tui }, typed.resolve));
      return typed.promise;
    },
    notice: (text, tone) => { notices.push({ text, tone }); },
    cards: {
      watch: (id, askedAt, settled) => dialogs.watch(childQuestionCard(id, askedAt), settled),
      adopt: <T>(bind: (close: () => void) => void, open: () => T): T => adoption.with(bind, open),
    },
  });
  return { dialogs, prompts, answers, marks, notices, cards, ask: (askedAt: number) => { children = [asking(askedAt)]; } };
}

test("a child's question in a worker session is one card, answered from either side", async () => {
  const session = workerSession();
  const kinds = () => session.dialogs.entries().map(entry => entry.card.kind);

  // Answered in the terminal: the prompt registers no card of its own, and the host's card delivers and records it once.
  session.ask(10);
  session.prompts.question("scout");
  expect(kinds()).toEqual(["child_question"]);
  armed(() => { session.cards[0]!.handleInput("1"); session.cards[0]!.handleInput("2"); });
  await settle();
  expect(session.answers).toEqual(["scout SQLite"]);
  expect(session.marks.map(mark => `${mark.act} ${mark.text}`)).toEqual(["answered SQLite"]);
  expect(session.notices.at(-1)?.tone).toBe("success");
  expect(kinds()).toEqual([]);

  // Answered in the app: the terminal's prompt closes with one quiet notice, and its keys do nothing afterwards.
  session.ask(20);
  session.prompts.question("scout");
  expect(kinds()).toEqual(["child_question"]);
  const card = session.dialogs.entries()[0]!.card;
  expect(await session.dialogs.resolve(card.id, [{ options: [], text: "Both" }], false, { source: "remote", device: "phone" })).toBe("delivered");
  await settle();
  armed(() => { session.cards[1]!.handleInput("2"); session.cards[1]!.handleInput("\r"); });
  await settle();
  expect(session.answers).toEqual(["scout SQLite", "scout Both"]);
  expect(session.marks.map(mark => `${mark.act} ${mark.text}`)).toEqual(["answered SQLite", "answered Both"]);
  expect(session.notices.filter(notice => notice.text.includes("another device"))).toEqual([{ text: "Answered from another device", tone: "text" }]);
  expect(session.cards).toHaveLength(2);
  expect(kinds()).toEqual([]);
});
