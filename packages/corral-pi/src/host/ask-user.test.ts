import { expect, test } from "bun:test";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { askUserEnabled, askUserTool } from "./ask-user.ts";

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
