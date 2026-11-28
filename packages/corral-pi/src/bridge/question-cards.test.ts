import { expect, test } from "bun:test";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { askUserTool } from "../host/ask-user.ts";
import { PendingQuestions } from "../host/pending-questions.ts";
import { questionEntries, registerCards, type SessionCard } from "../host/remote/cards.ts";
import { RemoteDialogs } from "../host/remote/dialogs.ts";
import { RemoteRegistry } from "../host/remote/registry.ts";

function bridge(dialogs: RemoteDialogs) {
  const registry = new RemoteRegistry();
  registerCards(registry, { dialogs, ready: () => true, beforeQuestion: async () => {} });
  return {
    async card() {
      const { cards } = await registry.get("cards")!({}) as { cards: SessionCard[] };
      return cards[0]!;
    },
    answer: async (card: string, answers: unknown[], dismiss = false) => registry.get("card_answer")!({ card, answers, dismiss }),
  };
}

test("the question bridge preserves pick notes and Other independently, without counting dismissal as consent", async () => {
  const dialogs = new RemoteDialogs(() => {});
  const { card, answer } = bridge(dialogs);
  const recorded: string[] = [];
  const ctx = { hasUI: true, ui: {
    custom: (factory: (...args: unknown[]) => void) => new Promise(resolve => factory({ requestRender() {} }, {}, {}, resolve)),
  } } as unknown as ExtensionToolContext;
  const tool = askUserTool(async text => { recorded.push(text); return text; }, undefined,
    undefined, undefined, undefined, undefined, () => dialogs);
  const params = { questions: [
    { question: "Scope?", options: ["A", "B"], freeText: true },
    { question: "Which lanes?", options: ["C", "D"], multiSelect: true, freeText: true },
    { question: "Which path?", options: ["E", "F"], multiSelect: true },
  ] };
  try {
    const result = tool.execute("call", params, undefined, undefined, ctx);
    const shown = await card();
    expect(shown.questions.map(q => [q.free_text, q.note_on_picks ?? false])).toEqual([[true, true], [true, true], [true, false]]);
    const picksWithNotes = [{ options: ["A"], text: "within budget" }, { options: ["C", "D"], text: "in order" }];
    await expect(answer(shown.id, [...picksWithNotes, { options: ["E"], text: "not a note" }])).rejects.toThrow("free text");
    expect(await answer(shown.id, [...picksWithNotes, { text: "another path" }])).toEqual({ status: "answered" });
    expect((await result).details).toMatchObject({ status: "answered", answers: [
      { question: "Scope?", option: "A", text: "within budget" },
      { question: "Which lanes?", option: "C, D", options: ["C", "D"], text: "in order" },
      { question: "Which path?", text: "another path" },
    ] });

    const cancelled = tool.execute("cancel", params, undefined, undefined, ctx);
    expect(await answer((await card()).id, [], true)).toEqual({ status: "dismissed" });
    expect((await cancelled).details).toEqual({ status: "no answer" });
    expect(recorded).toHaveLength(1);
  } finally {
    dialogs.close();
  }
});

test("deferred question cards advertise only the pick notes their answers accept", async () => {
  const pending = new PendingQuestions({ session: () => "test", entries: () => [], append() {}, deliver: async () => true, changed() {} });
  const dialogs = new RemoteDialogs(() => {}, { entries: () => questionEntries(pending), absent: () => "missing", shared: () => true });
  const { card, answer } = bridge(dialogs);
  try {
    pending.ask("call", [
      { question: "Scope?", options: ["A", "B"], freeText: true },
      { question: "Path?", options: ["C", "D"] },
      { question: "Proceed?", options: ["Yes", "No"], freeText: false },
    ]);
    const shown = await card();
    expect(shown.questions.map(q => [q.free_text, q.note_on_picks ?? false])).toEqual([[true, true], [true, false], [false, false]]);
    const answers = [{ options: ["A"], text: "within budget" }, { text: "another path" }, { options: ["No"] }];
    expect(await answer(shown.id, answers, true)).toEqual({ status: "dismissed" });
    expect(pending.get(shown.id)?.status).toBe("pending");
    expect(await answer(shown.id, answers)).toEqual({ status: "answered" });
    expect(pending.get(shown.id)?.answers).toEqual([{ option: "A", text: "within budget" }, { text: "another path" }, { option: "No" }]);
  } finally {
    dialogs.close();
    pending.close();
  }
});
