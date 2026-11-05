import { expect, test } from "bun:test";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { PendingQuestions, QUESTION_ENTRY, type QuestionEntry } from "../host/pending-questions.ts";
import { OwnerQuestionUi, QUESTIONS_SHORTCUT } from "./owner-questions.ts";

test("owner UI dismisses without consent, preserves permission dialogs and fences stale callbacks", async () => {
  const entries: QuestionEntry[] = [];
  let records = 0, selects = 0;
  const q = new PendingQuestions({ session: () => "owned", generation: 1, entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); }, changed() {},
    record: async text => { records++; return text; }, deliver: async () => true });
  const handlers = new Map<string, (event: unknown, ctx: ExtensionContext) => unknown>();
  let command!: (args: string, ctx: ExtensionContext) => Promise<void>;
  const shortcuts: string[] = [];
  const api = {
    on: (name: string, handler: (event: unknown, ctx: ExtensionContext) => unknown) => { handlers.set(name, handler); },
    registerCommand: (_name: string, spec: { handler: typeof command }) => { command = spec.handler; },
    registerShortcut: (name: string) => { shortcuts.push(name); },
  } as unknown as ExtensionAPI;
  let select: () => Promise<string | undefined> = async () => undefined;
  const ctx = { mode: "tui", hasUI: true, ui: { setWidget() {}, setStatus() {}, notify() {},
    select: async () => { selects++; return select(); },
  } } as unknown as ExtensionContext;
  const ui = new OwnerQuestionUi(q, () => true); ui.install(api);
  await handlers.get("session_start")?.({}, ctx);
  const id = q.ask("Q", [{ question: "Which?", options: ["One", "Two"] }]).id;
  expect(shortcuts).toContain(QUESTIONS_SHORTCUT);
  await command(id, ctx); // Escape or timeout from an owned answer UI.
  expect(q.get(id)?.status).toBe("pending"); expect(records).toBe(0);
  handlers.get("ui_prompt_start")?.({}, ctx); // An existing owner-action or permission dialog.
  await command(id, ctx);
  expect(selects).toBe(1);
  handlers.get("ui_prompt_end")?.({}, ctx);
  let release!: (answer: string) => void, opened!: () => void;
  const opening = new Promise<void>(resolve => { opened = resolve; });
  select = () => { opened(); return new Promise(resolve => { release = resolve; }); };
  const late = command(id, ctx); await opening;
  ui.close(); release("One"); await late;
  expect(records).toBe(0); expect(q.get(id)?.status).toBe("pending");
  q.restore();
  expect(q.cancel(id)).toBe("cancelled");
  expect(await q.answer(id, [{ option: "One" }])).toBe("cancelled");
  expect(records).toBe(0);
});
