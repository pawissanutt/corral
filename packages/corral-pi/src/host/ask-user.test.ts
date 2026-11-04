import { expect, test } from "bun:test";
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
