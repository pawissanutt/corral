import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { Check } from "typebox/value";

import { SUMMARY_MAX_BYTES, disposition, summarize, summaryText } from "../src/host/request.ts";
import { assetRoot, readAsset } from "../src/roles/common/assets.ts";
import { REQUEST_ASSET, REQUEST_OFF_ASSET, sessionTodoTool } from "../src/roles/common/todo.ts";
import assistant from "../src/roles/assistant/index.ts";
import pm from "../src/roles/pm/index.ts";
import { ProgressSchema, TodoWidget } from "../src/ui/todo-widget.ts";

const cases = JSON.parse(readFileSync(new URL("./fixtures/disposition-cases.json", import.meta.url), "utf8")) as
  { name: string; progress: unknown; disposition: unknown }[];
const theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };

for (const item of cases) {
  test(`disposition mirrors corral-core: ${item.name}`, () => {
    // Given the case corral-core's request_tests also reads.
    const progress = item.progress === null ? null : ProgressSchema.parse(item.progress);
    // Then the host derives the same machine value.
    expect(disposition(progress)).toEqual(item.disposition as ReturnType<typeof disposition>);
  });
}

function plan(open: number) {
  return ProgressSchema.parse({
    revision: 9, spec_revision: "pm:", plan: "P3",
    phases: [{ id: "p", label: "Work", steps: [
      { id: "done", label: "Finished", status: { state: "done" } },
      ...Array.from({ length: open }, (_, n) => ({
        id: `s${n}`, label: `${"Long step label ".repeat(20)}\u001b[31m${n}`, status: { state: n === 0 ? "in_progress" : "pending" },
      })),
    ] }],
    waits: [{ step: "s1", on: { kind: "owner", question: "Which project?" } }],
  });
}

test("the request summary is bounded and names the plan, revision, outstanding steps and next action", () => {
  // Given a plan with many long open steps.
  const summary = summarize(plan(40), null);
  // Then the machine fields carry the plan identity, the next action and a bounded step list.
  expect(summary?.plan).toBe("P3");
  expect(summary?.revision).toBe(9);
  expect(summary?.disposition).toEqual({ state: "actionable", next: "s0" });
  expect(summary?.next?.id).toBe("s0");
  expect(summary?.outstanding).toHaveLength(8);
  expect(summary?.omitted).toBe(32);
  expect(summary?.outstanding[1]?.wait).toContain("Which project?");
  // And its text fits the budget without terminal controls.
  const text = summaryText(summary!);
  expect(Buffer.byteLength(text)).toBeLessThanOrEqual(SUMMARY_MAX_BYTES);
  expect(text).not.toContain("\u001b");
  expect(text).toContain("P3 revision 9");
  expect(summarize(null, null)).toBeNull();
});

test("a stopped continuation is reported only for the revision it stopped", () => {
  // Given the ledger's continuation record for revision 9, then for an older one.
  const progress = plan(2);
  const stopped = { key: "continue:P3:9", plan: "P3", revision: 9, attempt: 2, stopped: "no_progress" as const };
  // Then only the current revision's stop reaches the summary.
  expect(summarize(progress, stopped)?.stopped).toBe("no_progress");
  expect(summarize(progress, { ...stopped, revision: 8 })?.stopped).toBeNull();
});

test("the widget and the model summary name the same plan identity and revision", () => {
  // Given one canonical ledger.
  const progress = plan(2);
  const widget = new TodoWidget(theme, () => 40, () => {});
  widget.update(progress);
  // Then the widget header and the summary both say P3 at revision 9.
  expect(widget.render(80)[0]).toContain("P3 r9");
  expect(summaryText(summarize(progress, null)!)).toContain("P3 revision 9");
});

test("waits are visible in the expanded widget", () => {
  const widget = new TodoWidget(theme, () => 80, () => {});
  widget.update(plan(2));
  widget.toggle();
  expect(widget.render(200).join("\n")).toContain("Which project?");
});

const variants = [
  { op: "init", phases: [{ id: "p", label: "Work", steps: [{ id: "a", label: "A" }] }] },
  { op: "append", phase: "fix", label: "Correction", steps: [{ id: "b", label: "B" }], plan: "P1" },
  { op: "start", step: "a", plan: "P1" },
  { op: "done", step: "a" },
  { op: "drop", step: "a", reason: "Superseded", plan: "P1" },
  { op: "wait", step: "a", on: { kind: "event", event: "T-12 lands" } },
  { op: "wait", step: "a", on: { kind: "owner", question: "Which?" }, plan: "P1" },
];

for (const payload of variants) {
  test(`session todo ${payload.op} validates and preserves its canonical payload`, () => {
    expect(Check(sessionTodoTool.parameters, payload)).toBeTrue();
    expect(sessionTodoTool.payload(payload)).toEqual(payload);
    const foreign = { ...payload, task: "T-1" };
    expect(Check(sessionTodoTool.parameters, foreign)).toBeFalse();
  });
}

test("session todo refuses criteria and malformed waits", () => {
  for (const payload of [
    { op: "init", phases: [{ id: "p", label: "Work", steps: [{ id: "a", label: "A", criteria: ["AC-1"] }] }] },
    { op: "wait", step: "a", on: { kind: "event" } },
    { op: "wait", step: "a", on: { kind: "owner", event: "x" } },
    { op: "wait", step: "a" },
  ]) expect(Check(sessionTodoTool.parameters, payload)).toBeFalse();
});

for (const role of [pm, assistant]) {
  for (const todo of [true, false, undefined]) {
    test(`${role.role} todo=${String(todo)} carries the matching request-execution contract`, () => {
      // Given the role's launch-authorized todo setting.
      const context = {
        identity: { store: "s", project: "", role: role.role, generation: 1, assistant: "A-1" },
        cwd: "/tmp", model: "m", ...(todo === undefined ? {} : { todo }),
      };
      const root = assetRoot(new URL("../src/roles/pm/index.ts", import.meta.url).href);
      const contract = role.contract(context);
      // Then the contract ships the enabled section only with the tool, never both.
      const on = readAsset(root, REQUEST_ASSET);
      const off = readAsset(root, REQUEST_OFF_ASSET);
      expect(contract.includes(on)).toBe(todo === true);
      expect(contract.includes(off)).toBe(todo !== true);
      expect(role.tools(context).some((tool) => tool.name === "corral_todo")).toBe(todo === true);
    });
  }
}
