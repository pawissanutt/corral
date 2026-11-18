import { beforeAll, expect, test } from "bun:test";

import { type AgentToolResult, initTheme } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";

import { artifactTool } from "../roles/common/tools.ts";
import { sessionTodoTool, todoTool } from "../roles/common/todo.ts";
import { verdictTool } from "../roles/reviewer/tools.ts";
import { checkpointTool, doneTool } from "../roles/worker/tools.ts";
import { CORRAL_TOOL_OPS, corralPresenter, corralSummary } from "./corral-presenter.ts";
import { rowBlock, type CallInput, type RowModel } from "./tool-row.ts";

beforeAll(() => initTheme("dark", false));

function call(name: string, args: Record<string, unknown>, result?: AgentToolResult<unknown>, isError = false): CallInput {
  return { name, args, argsComplete: true, executionStarted: true, partial: false, isError, expanded: false, cwd: "/w", toolCallId: "t1",
    ...(result ? { result } : {}) };
}
/** A result as define.ts returns it: the JSON text, with the value as details unless a Code Mode child carries text only. */
function result(value: unknown, details: unknown = value): AgentToolResult<unknown> {
  return { content: [{ type: "text", text: JSON.stringify(value) }], details };
}
function present(input: CallInput, opened: string[] = []): RowModel {
  const presenter = corralPresenter(input.name, { openView: view => { opened.push(view); } });
  if (!presenter) throw new Error(`no presenter for ${input.name}`);
  return presenter(input);
}
function drawn(model: RowModel, width: number, expanded = false): string {
  return rowBlock(model, width, 0, expanded).lines.map(line => stripTerminalSequences(line)).join("\n");
}
const progress = {
  revision: 2, spec_revision: "T-1", plan: "P1", phases: [{ id: "p", label: "Build", steps: [
    { id: "a", label: "Write the parser", status: { state: "done" }, criteria: [] },
    { id: "b", label: "Wire the panel", status: { state: "in_progress" }, criteria: [] },
  ] }],
};
const ledger = { revision: 2, progress, plan: "P1", disposition: { state: "actionable", next: "b" }, closed_plans: 0 };

test("a successful call reads the result itself, and every corral tool maps to its operation", () => {
  for (const spec of [doneTool, verdictTool, artifactTool, todoTool, sessionTodoTool, checkpointTool]) expect(CORRAL_TOOL_OPS[spec.name]).toBe(spec.op);
  const opened: string[] = [];
  const step = present(call("corral_todo", { op: "done", step: "a" }, result(ledger)), opened);
  expect(step).toMatchObject({ state: "ok", verb: "Finished step", target: "Write the parser", outcome: "plan 1/2" });
  step.view?.open();
  expect(opened).toEqual(["plan"]);
  const summary = `${"Implemented and documented the reconciliation, rebased onto main. ".repeat(9)}Verified: focused tests. Not verified: full gate.`;
  const done = present(call("corral_done", { summary }, result({ id: "T-1", status: "review" })));
  const collapsed = drawn(done, 100).split("\n");
  expect(collapsed[0]).toContain("Finished the run");
  expect(collapsed[0]).toContain("sent to review");
  expect(collapsed.length).toBeLessThanOrEqual(4);
  expect(drawn(done, 100, true).replace(/\s+/g, " ")).toContain("Verified: focused tests. Not verified: full gate.");
});

test("an engine refusal reads refused with its reason, and its next commands open one per line", () => {
  const failure = { reason: "T-1 is not running", next: ["corral start T-1", "corral show T-1"] };
  const refused = { content: [{ type: "text" as const, text: "refused: T-1 is not running\nnext: corral start T-1\nnext: corral show T-1" }],
    details: failure };
  const model = present(call("corral_done", { summary: "Implemented the change." }, refused, true));
  expect(model).toMatchObject({ state: "failed", verb: "Finish", outcome: "refused" });
  for (const width of [100, 46]) {
    expect(drawn(model, width)).toContain("T-1 is not running");
    const expanded = drawn(model, width, true).split("\n");
    for (const next of failure.next) expect(expanded.filter(line => line.trim() === `next: ${next}`)).toHaveLength(1);
  }
  expect(corralSummary("corral_done", { summary: "x" }, refused, true)).toBe("✗ Finish · refused: T-1 is not running");
});

test("a schema refusal is not sent, names the field and never reads meaning into the arguments", () => {
  const pi = { content: [{ type: "text" as const, text: "Validation failed for tool \"corral_todo\":\n  - op: expected one of [\"init\",\"done\"]\n\nReceived arguments:\n{}" }], details: {} };
  const model = present(call("corral_todo", { op: "wait", step: "b" }, pi, true));
  expect(model).toMatchObject({ state: "failed", outcome: "not sent" });
  expect(model.target).toBeUndefined();
  expect(model.body?.[0]?.text).toContain("op: expected one of");
  const host = result({ reason: "Invalid arguments: /path" });
  expect(present(call("corral_artifact", { path: 3 }, host, true))).toMatchObject({ state: "failed", outcome: "not sent" });
});

test("a Code Mode child carrying only the result text reads that JSON, success or refusal", () => {
  const step = present(call("corral_todo", { op: "done", step: "a" }, result(ledger, {})));
  expect(step).toMatchObject({ state: "ok", target: "Write the parser", outcome: "plan 1/2" });
  const refused = present(call("corral_todo", { op: "start", step: "a" }, result({ reason: "terminal_todo_step" }, {}), true));
  expect(refused).toMatchObject({ state: "failed", verb: "Plan update", outcome: "refused" });
  expect(refused.body?.[0]?.text).toBe("that step is already finished");
});

test("prose streams its newest rows while the arguments are written", () => {
  const note = Array.from({ length: 12 }, (_, i) => `Line ${i} of the resume note.`).join("\n");
  const model = present({ ...call("corral_checkpoint", { note }), argsComplete: false, executionStarted: false });
  expect(model).toMatchObject({ state: "writing", verb: "Saving a checkpoint", outcome: `${note.length} chars` });
  const text = drawn(model, 100);
  expect(text).toContain("Line 11 of the resume note.");
  expect(text).not.toContain("Line 0 of");
});

test("a steer that was not delivered reads as not delivered", () => {
  const reply = { content: [{ type: "text" as const, text: "Child api is not running; no message delivered." }], details: { delivered: false } };
  const model = present(call("agent_message", { id: "api", text: "Use the existing type." }, reply));
  expect(model.state).toBe("failed");
  expect(model.outcome).toBe("not delivered");
});
