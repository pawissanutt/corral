import { expect, test } from "bun:test";
import { Check } from "typebox/value";
import { Type } from "typebox";
import { stream } from "@earendil-works/pi-ai/api/anthropic-messages";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
import { normalizeContext } from "@earendil-works/pi-ai/utils/transcript";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { askUserEnabled, askUserTool } from "./ask-user.ts";
import { AskDialog, type DialogResult } from "./ask-user-dialog.ts";

test("only interactive roles can enable the dialog in the host loadout", () => {
  for (const role of ["assistant", "pm"]) {
    expect(askUserEnabled(role, ["ask_user"])).toBeTrue();
    expect(askUserEnabled(role, [])).toBeFalse();
  }
  for (const role of ["worker", "reviewer", "planner", "plan-reviewer"]) {
    expect(askUserEnabled(role, ["ask_user"])).toBeFalse();
  }
});

const params = { questions: [{ question: "Which?", options: ["One", "Two"], freeText: true }] };

test("non-blocking clarification returns pending without opening a modal", async () => {
  const ctx = { hasUI: true, mode: "tui", ui: { select: async () => { throw new Error("modal opened before owner requested it"); } } } as unknown as ExtensionToolContext;
  const pending = { ask: () => ({ version: 1 as const, id: "Q1", status: "pending" as const,
    session: "test", generation: 1, key: "id", questions: params.questions, expires: 123 }), withdraw: () => "withdrawn" };
  const tool = askUserTool(async () => { throw new Error("no answer exists"); }, pending);
  const [model] = anthropicProvider().getModels();
  if (!model) throw new Error("Anthropic fixture model unavailable");
  let payload: unknown;
  let network = 0;
  await stream(model, normalizeContext({ messages: [
    { role: "system", content: "Offline schema check", toolsAdded: [tool], timestamp: 0 },
    { role: "user", content: "Ask a question", timestamp: 1 },
  ] }), {
    apiKey: "offline-placeholder",
    onPayload: value => { payload = value; throw new Error("OFFLINE_CAPTURE_ONLY"); },
    fetch: async () => { network++; throw new Error("NETWORK_FORBIDDEN"); },
  }).result();
  expect(network).toBe(0);
  expect(Check(Type.Object({ tools: Type.Array(Type.Unknown(), { contains: Type.Object({
    name: Type.Literal("ask_user"), input_schema: Type.Object({ type: Type.Literal("object"),
      properties: Type.Object({ questions: Type.Object({}), nonBlocking: Type.Object({}), withdraw: Type.Object({}) }),
    }),
  }) }) }), payload)).toBeTrue();
  const result = await tool.execute("id", { ...params, nonBlocking: true }, undefined, undefined, ctx);
  expect(result.details).toMatchObject({ id: "Q1", status: "pending" });
  const rich = { questions: [{ question: "Which?", options: [{ label: "One", description: "First" }, "Two"], multiSelect: true }] };
  expect(Check(tool.parameters, rich)).toBeTrue();
  expect(Check(tool.parameters, { ...rich, nonBlocking: true })).toBeFalse();
  await expect(tool.execute("rich", { ...rich, nonBlocking: true }, undefined, undefined, ctx))
    .rejects.toThrow("deferred_questions_require_plain_single_selection");
});

test("dialog presents options and returns and records the owner's choice and text", async () => {
  const seen: unknown[] = [];
  const recorded: string[] = [];
  const ctx = { hasUI: true, ui: {
    select: async (title: string, options: string[]) => { seen.push(title, options); return "Two"; },
    input: async () => "Extra detail",
  } } as unknown as ExtensionToolContext;
  const result = await askUserTool(async (answer) => { recorded.push(answer); return answer; }).execute("id", params, undefined, undefined, ctx);
  expect(seen).toEqual(["Which?", ["One", "Two"]]);
  expect(result.content).toEqual([{ type: "text", text: "Which?: Two\nExtra detail" }]);
  expect(recorded).toEqual(["Which?: Two\nExtra detail"]);
});

test("cancel or timeout at either dialog is no answer and records no consent", async () => {
  const recorded: string[] = [];
  const ctx = { hasUI: true, ui: { select: async () => undefined } } as unknown as ExtensionToolContext;
  const result = await askUserTool(async (answer) => { recorded.push(answer); return answer; }).execute("id", params, undefined, undefined, ctx);
  expect(result.content).toEqual([{ type: "text", text: "no answer" }]);
  const duringText = { hasUI: true, ui: { select: async () => "One", input: async () => undefined } } as unknown as ExtensionToolContext;
  const second = await askUserTool(async (answer) => { recorded.push(answer); return answer; })
    .execute("id", params, undefined, undefined, duringText);
  expect(second.content).toEqual([{ type: "text", text: "no answer" }]);
  for (const unavailable of [{ hasUI: false, mode: "print" }, { hasUI: true, mode: "rpc" }]) {
    const result = await askUserTool(async answer => { recorded.push(answer); return answer; })
      .execute("id", { ...params, nonBlocking: true }, undefined, undefined, unavailable as unknown as ExtensionToolContext);
    expect(result.details).toMatchObject({ status: "no answer" });
  }
  const busy = await askUserTool(async () => "not called", undefined, () => true)
    .execute("busy", params, undefined, undefined, duringText);
  expect(busy.details).toMatchObject({ status: "no answer", reason: "another owner/permission dialog is open" });
  expect(recorded).toEqual([]);
});

test("the batch dialog keeps answered questions when the owner escapes and lists the unanswered", async () => {
  const recorded: string[] = [];
  const batch = { questions: [
    { question: "Scope?", header: "Scope", options: [{ label: "Small", description: "one crate" }, "Large"] },
    { question: "Lanes?", options: ["Quick", "Full", "Slow"], multiSelect: true },
  ] };
  const ctx = { hasUI: true, ui: {
    custom: async (factory: (...args: unknown[]) => { handleInput(data: string): void; render(width: number): string[] }) => {
      let result: unknown;
      const dialog = factory({ requestRender() {} }, {}, {}, (r: unknown) => { result = r; });
      expect(dialog.render(60).join("\n")).toContain("one crate");
      dialog.handleInput("\x1b[B"); // down to Large
      dialog.handleInput("\r"); // answer Scope, move to Lanes
      dialog.handleInput("\x1b"); // escape without answering Lanes
      return result;
    },
  } } as unknown as ExtensionToolContext;
  const result = await askUserTool(async (answer) => { recorded.push(answer); return answer; }).execute("id", batch, undefined, undefined, ctx);
  expect(result.details).toEqual({ status: "partial", answers: [{ question: "Scope?", option: "Large" }], unanswered: ["Lanes?"] });
  expect(recorded).toEqual(["Scope?: Large\nUnanswered (no consent): Lanes?"]);
});

test("a pasted note survives revision and a partial Escape", () => {
  let result: DialogResult | undefined;
  const dialog = new AskDialog([{ question: "Which?", options: [{ label: "One" }, { label: "Two" }], freeText: true }], (r) => { result = r; });
  dialog.handleInput("\r"); // choose One; the only question, so Submit
  dialog.handleInput("\x1b[D"); // back to the question to revise
  dialog.handleInput("\x1b[B"); dialog.handleInput("\x1b[B"); // down to Note
  dialog.handleInput("\x1b[200~keep database\nunchanged\x1b[201~");
  dialog.handleInput("!");
  dialog.handleInput("\x1b");
  expect(result).toEqual({ submitted: false, answers: [{ selected: ["One"], text: "keep database unchanged!" }] });
});
