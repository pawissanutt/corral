import { expect, test } from "bun:test";
import { SessionManager, type BeforeAgentStartEvent, type BeforeAgentStartEventResult, type ContextEvent, type ContextEventResult,
  type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent";
import { buildSystemPrompt, normalizeBuildSystemPromptOptions } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/system-prompt.js";
import { PendingQuestions, QUESTION_ENTRY, type QuestionEntry } from "../host/pending-questions.ts";
import { OwnerQuestionUi, OWNER_QUESTION_STATE, QUESTIONS_SHORTCUT, renderOwnerQuestionState } from "./owner-questions.ts";

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
  const ui = new OwnerQuestionUi(q, () => true, () => {}); ui.install(api);
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

test("owner question changes stay in host messages without changing the default system prefix or duplicating state", async () => {
  const entries: QuestionEntry[] = [];
  const questions = new PendingQuestions({ session: () => "owned", generation: 1, entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); }, changed() {},
    record: async text => text, deliver: async () => false });
  const sessionManager = SessionManager.inMemory("/project");
  const ctx = { sessionManager, hasUI: false } as unknown as ExtensionContext;
  let before!: (event: BeforeAgentStartEvent, ctx: ExtensionContext) => BeforeAgentStartEventResult | void;
  let context!: (event: ContextEvent, ctx: ExtensionContext) => ContextEventResult | void;
  const handlers = new Map<string, unknown>();
  const api = {
    on: (name: string, handler: unknown) => {
      handlers.set(name, handler);
      if (name === "before_agent_start") before = handler as typeof before;
      if (name === "context") context = handler as typeof context;
    },
    registerCommand() {}, registerShortcut() {},
  } as unknown as ExtensionAPI;
  let claude = false;
  const journal = (message: { customType: string; content: string; display: boolean }) => {
    sessionManager.appendCustomMessageEntry(message.customType, message.content, message.display, undefined);
  };
  new OwnerQuestionUi(questions, () => false, journal, () => claude).install(api);
  const start = () => {
    const options = normalizeBuildSystemPromptOptions({ cwd: "/project", appendSystemPrompt: "Stable role contract" });
    const event = { type: "before_agent_start", messageType: "user", prompt: "continue",
      systemPromptOptions: options, systemPrompt: buildSystemPrompt(options) } as BeforeAgentStartEvent;
    const result = before(event, ctx);
    if (result?.message) {
      const message = result.message;
      sessionManager.appendCustomMessageEntry(message.customType, message.content, message.display, message.details);
    }
    return { system: buildSystemPrompt(options), message: result?.message, options };
  };
  const id = questions.ask("Q", [{ question: "Which release?", options: ["One", "Two"] }]).id;
  const pending = start();
  expect(pending.system).not.toContain("Which release?");
  expect(pending.options.sections.owner_questions).toBeUndefined();
  expect(pending.message?.customType).toBe(OWNER_QUESTION_STATE);
  expect(pending.message?.content).toContain(renderOwnerQuestionState(questions));
  expect(start().message).toBeUndefined();
  // Reload/resume must inspect retained context rather than reset an in-memory flag.
  new OwnerQuestionUi(questions, () => false, journal, () => claude).install(api);
  await (handlers.get("session_start") as (event: unknown, ctx: ExtensionContext) => unknown)({}, ctx);
  expect(start().message).toBeUndefined();
  const kept = sessionManager.appendMessage({ role: "user", content: "continue", timestamp: 0 });
  sessionManager.appendCompaction("Summary without authoritative host state", kept, 1);
  const compacted = sessionManager.buildSessionProjection().messages;
  expect(compacted.some(message => message.role === "custom" && message.customType === OWNER_QUESTION_STATE)).toBeFalse();
  // Automatic continuations skip before_agent_start: the first request restores state and journals it.
  const restored = context({ type: "context", messages: compacted }, ctx)?.messages;
  expect(restored?.slice(0, -1)).toEqual(compacted);
  const restoredState = restored?.at(-1);
  expect(restoredState?.role === "custom" && restoredState.content).toContain(renderOwnerQuestionState(questions));
  // The journal holds that message, so the next request repeats it in place and adds none. A
  // request-only copy moved on every request and stopped the provider cache from growing.
  const next = sessionManager.buildSessionProjection().messages;
  expect(next.filter(message => message.role === "custom" && message.customType === OWNER_QUESTION_STATE)).toHaveLength(1);
  expect(context({ type: "context", messages: next }, ctx)).toBeUndefined();
  expect(start().message).toBeUndefined();
  await questions.answer(id, [{ option: "One" }]);
  expect(questions.status(id)).toBe("answered");
  const answered = start();
  expect(answered.system).toBe(pending.system);
  expect(answered.message?.content).toContain(renderOwnerQuestionState(questions));
  expect(answered.message?.content).not.toBe(pending.message?.content);
  expect(start().message).toBeUndefined();
  questions.cancel(questions.ask("Other", [{ question: "Other?", options: ["Yes", "No"] }]).id);
  // The rendered pending/answered state is unchanged by a cancelled question.
  expect(start().message).toBeUndefined();
  claude = true;
  const routed = start();
  expect(routed.message).toBeUndefined();
  expect(routed.options.sections.owner_questions).toBe(renderOwnerQuestionState(questions));
});

test("/questions takes the owner's own typed answer, keeps drafts across questions and Escape sends nothing", async () => {
  const entries: QuestionEntry[] = [];
  const records: string[] = [];
  const q = new PendingQuestions({ session: () => "owned", generation: 1, entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); }, changed() {},
    record: async text => { records.push(text); return text; }, deliver: async () => true });
  const handlers = new Map<string, (event: unknown, ctx: ExtensionContext) => unknown>();
  let command!: (args: string, ctx: ExtensionContext) => Promise<void>;
  const api = {
    on: (name: string, handler: (event: unknown, ctx: ExtensionContext) => unknown) => { handlers.set(name, handler); },
    registerCommand: (_name: string, spec: { handler: typeof command }) => { command = spec.handler; },
    registerShortcut() {},
  } as unknown as ExtensionAPI;
  let keys: string[] = [];
  let screen = "";
  const screens: string[] = [];
  const ctx = { mode: "tui", hasUI: true, ui: { setWidget() {}, setStatus() {}, notify() {},
    select: async () => { throw new Error("select fallback used"); },
    custom: async (factory: (...args: unknown[]) => { handleInput(data: string): void; render(width: number): string[] }) => {
      let result: unknown;
      const dialog = factory({ requestRender() {} }, {}, {}, (r: unknown) => { result = r; });
      for (const key of keys) {
        dialog.handleInput(key); if (result) break;
        screen = dialog.render(72).join("\n"); screens.push(screen);
      }
      return result;
    },
  } } as unknown as ExtensionContext;
  new OwnerQuestionUi(q, () => true, () => {}).install(api);
  await handlers.get("session_start")?.({}, ctx);
  const id = q.ask("Q", [{ question: "Which release?", options: ["One", "Two"] },
    { question: "Notify?", options: ["Yes", "No"], freeText: false }]).id;
  const down = "\x1b[B", right = "\x1b[C", left = "\x1b[D";
  // Typing on Other, moving to Q2 and back keeps the draft; Escape leaves the question pending.
  keys = [down, down, "ship 1.4", right, left, "\x1b"];
  await command(id, ctx);
  expect(screen).toContain("Other: ship 1.4");
  expect(q.get(id)?.status).toBe("pending"); expect(records).toEqual([]);
  // A typed answer not among the options; Q2 forbids free text, so it has no Other row.
  keys = [down, down, "ship 1.4 only after QA", "\r", down, down, "x", "\r", "\r"];
  screens.length = 0;
  await command(id, ctx);
  const q2 = screens.filter(s => s.includes("Notify?"));
  expect(q2.length).toBeGreaterThan(0);
  for (const s of q2) expect(s).not.toContain("Other:");
  expect(screen).toContain("Q1: Other: ship 1.4 only after QA");
  expect(q.get(id)?.answers).toEqual([{ text: "ship 1.4 only after QA" }, { option: "No" }]);
  expect(records).toEqual([`[ask_user answer ${id}]\nWhich release?: ship 1.4 only after QA\nNotify?: No`]);
  expect(await q.answer(id, [{ text: "again" }, { option: "No" }])).not.toBe("pending");
  expect(records).toHaveLength(1);
});
