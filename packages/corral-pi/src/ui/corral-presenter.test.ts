import { beforeAll, expect, test } from "bun:test";

import { type AgentToolResult, initTheme } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";

import { CORRAL_OPS, type CorralToolSpec } from "../roles/types.ts";
import { assistantWorkflowTools } from "../roles/assistant/planning.ts";
import { ownerActionTool } from "../roles/assistant/owner-action.ts";
import { contentReadTool } from "../roles/common/content.ts";
import { memoryTools } from "../roles/common/memory.ts";
import { sessionTodoTool, todoTool } from "../roles/common/todo.ts";
import { artifactTool, askTool, evidenceTool, receiptLookupTool, withdrawTool } from "../roles/common/tools.ts";
import { planVerdictTool } from "../roles/plan-reviewer/tools.ts";
import { planSubmitTool, planWriteTool } from "../roles/planner/tools.ts";
import { pmTools } from "../roles/pm/tools.ts";
import { verdictTool } from "../roles/reviewer/tools.ts";
import { checkpointTool, doneTool, gateTool, progressTool, waitTool } from "../roles/worker/tools.ts";
import { CORRAL_TOOL_OPS, corralPresenter, corralSummary, presentOperation } from "./corral-presenter.ts";
import { rowBlock, type CallInput, type RowModel } from "./tool-row.ts";

beforeAll(() => initTheme("dark", false));

function call(name: string, args: Record<string, unknown>, result?: AgentToolResult<unknown>, isError = false): CallInput {
  return { name, args, argsComplete: true, executionStarted: true, partial: false, isError, expanded: false, cwd: "/w", toolCallId: "t1",
    ...(result ? { result } : {}) };
}
function receipt(op: string, state: string, result: unknown, extra: Record<string, unknown> = {}): AgentToolResult<unknown> {
  return { content: [{ type: "text", text: `${op}: ${state}` }], details: { version: 1, request_id: "rq-1", op, state, result, ...extra } };
}
function present(name: string, input: CallInput): RowModel {
  const presenter = corralPresenter(name);
  if (!presenter) throw new Error(`no presenter for ${name}`);
  return presenter(input);
}
function drawn(model: RowModel, width: number, expanded = false): string {
  return rowBlock(model, width, 0, expanded).lines.map(line => stripTerminalSequences(line)).join("\n");
}

test("every corral operation has a row, and every role tool maps to its operation", () => {
  for (const op of CORRAL_OPS) {
    const model = presentOperation(op, call(op, {}, receipt(op, "committed", {})));
    expect(model.verb.length).toBeGreaterThan(0);
    expect(model.verb).not.toContain("_");
  }
  const specs: readonly Pick<CorralToolSpec, "name" | "op">[] = [
    ...pmTools, ...memoryTools, ...assistantWorkflowTools, receiptLookupTool(), contentReadTool, todoTool, sessionTodoTool, askTool,
    withdrawTool, evidenceTool, artifactTool, waitTool, progressTool, checkpointTool, gateTool, doneTool, verdictTool,
    planWriteTool("plan"), planSubmitTool, planVerdictTool, ownerActionTool,
  ];
  for (const spec of specs) expect(CORRAL_TOOL_OPS[spec.name]).toBe(spec.op);
});

test("a rejected receipt, which the host marks as an error, shows its reason", () => {
  const model = present("corral_ask", call("corral_ask", { kind: "clarification", text: "Which slot?" },
    receipt("ask", "rejected", { code: "question_limit", reason: "T-140/R2 already has a pending blocking question" }, { stored: false }), true));
  expect(model.state).toBe("failed");
  expect(model.outcome).toBe("refused");
  expect(drawn(model, 100)).toContain("T-140/R2 already has a pending blocking question");
});

test("a finish refused for a missing checkpoint reads as a refusal with its reason at 100 and 46 columns", () => {
  const error = { code: "call_failed", category: "uncertain_effect", effect: "unknown", exit_code: 1, signal: null, timed_out: false,
    reason: "call_failed: corral agent call --op done: exit=1, signal=null, timed_out=false; error: checkpoint_required: run R1; use corral run checkpoint T-1 --file <json>",
    diagnostic: "error: checkpoint_required: run R1; use corral run checkpoint T-1 --file <json>" };
  const result = { content: [{ type: "text" as const, text: "done: host error" }], details: { request_id: "rq-1", op: "done", error } };
  const model = present("corral_done", call("corral_done", { summary: "Implemented the change." }, result, true));
  expect(model.state).toBe("failed");
  for (const width of [100, 46]) {
    const text = drawn(model, width);
    expect(text).toContain("✗");
    expect(text).toContain("refused");
    expect(text).toContain("a checkpoint is required first");
  }
});

test("a gate that only started is waiting, and an unknown effect is not a failure", () => {
  const gate = present("corral_gate", call("corral_gate", {}, receipt("gate", "committed", { outcome: "started", attempt: "ag-1", adopted: false })));
  expect(gate.state).toBe("waiting");
  expect(drawn(gate, 100)).not.toContain("✓");
  const timeout = { content: [{ type: "text" as const, text: "done: host error" }], details: { request_id: "rq-2", op: "done", error: {
    code: "call_timeout", effect: "unknown", exit_code: null, signal: "SIGKILL", timed_out: true, reason: "call_timeout: corral agent call --op done", diagnostic: "" } } };
  const unknown = present("corral_done", call("corral_done", { summary: "x" }, timeout, true));
  expect(unknown.state).toBe("unknown");
  expect(corralSummary("corral_done", { summary: "x" }, timeout, true)?.startsWith("!")).toBeTrue();
});

test("a long done summary keeps the outcome on the row and is readable in full when expanded", () => {
  const summary = `${"Implemented and documented the reconciliation, rebased onto main. ".repeat(9)}Verified: focused tests. Not verified: full gate.`;
  expect(summary.length).toBeGreaterThan(600);
  const model = present("corral_done", call("corral_done", { summary }, receipt("done", "committed", { outcome: "review", run: "R1" })));
  const collapsed = drawn(model, 100).split("\n");
  expect(collapsed[0]).toContain("sent to review");
  expect(collapsed.length).toBeLessThanOrEqual(4);
  expect(drawn(model, 100, true).replace(/\s+/g, " ")).toContain("Verified: focused tests. Not verified: full gate.");
});

test("a schema refusal names the field and never reads meaning into the arguments", () => {
  const result = { content: [{ type: "text" as const, text: "Validation failed for tool \"corral_todo\":\n  - op: expected one of [\"init\",\"done\"]\n\nReceived arguments:\n{}" }], details: {} };
  const model = present("corral_todo", call("corral_todo", { action: "wait", wait: { kind: "owner" } }, result, true));
  expect(model.state).toBe("failed");
  expect(model.outcome).toBe("not sent");
  expect(model.target).toBeUndefined();
  expect(model.body?.[0]?.text).toContain("op: expected one of");
});

test("paged reads name their page from the call's offset, so a batch over one artifact reads row by row", () => {
  const model = present("corral_inbox_read", call("corral_inbox_read", { id: "PMI-59", offset: 1024, limit: 1024 },
    receipt("inbox_read", "committed", { content_json: "x".repeat(501), next_offset: null, total_chars: 1525, item: { kind: "handoff", from: "owner" } })));
  expect(model.target).toContain("page 2 of 2");
  // Code Mode children: a reduced receipt (scalars only) and offsets snapped to UTF-8 boundaries.
  const reference = "artifacts/T-949/R3/88153bf1-T-949-R3-3767185-1794368790486565526.log";
  const rows = [0, 4096, 8191, 12287].map(offset => present("corral_content_read", call("corral_content_read", { reference, offset },
    { content: [], details: { state: "committed", bytes: 13813, next_offset: offset + 4096 } })));
  expect(new Set(rows.map(row => row.target)).size).toBe(4);
  expect(rows[2]?.target).toContain("page 3 of 4");
  const narrow = drawn(rows[2] ?? { state: "ok", verb: "Read" }, 46).split("\n")[0] ?? "";
  expect(narrow).toContain("T-949/R3");
  expect(narrow).toContain("page 3 of 4");
});

test("a steer that was not delivered reads as not delivered", () => {
  const result = { content: [{ type: "text" as const, text: "Child api is not running; no message delivered." }], details: { delivered: false } };
  const model = present("agent_message", call("agent_message", { id: "api", text: "Use the existing type." }, result));
  expect(model.state).toBe("failed");
  expect(model.outcome).toBe("not delivered");
});
