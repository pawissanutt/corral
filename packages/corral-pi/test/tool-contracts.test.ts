import { expect, test } from "bun:test";
import { Check } from "typebox/value";
import { pmTools } from "../src/roles/pm/tools.ts";
import { askTool, evidenceTool, receiptLookupTool } from "../src/roles/common/tools.ts";
import { checkpointTool, progressTool, waitTool, doneTool } from "../src/roles/worker/tools.ts";
import { verdictTool } from "../src/roles/reviewer/tools.ts";
import { planWriteTool, planSubmitTool } from "../src/roles/planner/tools.ts";
import { planVerdictTool } from "../src/roles/plan-reviewer/tools.ts";
import { defineCorralTool, type ToolHost } from "../src/tools/define.ts";
import { CorralCallError, CorralClient, type CorralReceipt, toolRequestId } from "../src/tools/corral.ts";
import type { CorralToolSpec } from "../src/roles/types.ts";

const spec = (op: string) => pmTools.find((s) => s.op === op)!;
const valid = (op: string, args: unknown) => Check(spec(op).parameters, args);

test("PM list/search bounds, enums, revisions and conditional modes match Rust", () => {
  for (const limit of [1, 100]) {
    expect(valid("read_list", { limit, offset: 0, status: "needs_me" })).toBeTrue();
    expect(valid("read_search", { query: "x", limit })).toBeTrue();
    expect(valid("inbox_read", { limit })).toBeTrue();
  }
  for (const limit of [0, -1, 101, 1.5]) {
    expect(valid("read_list", { limit })).toBeFalse();
    expect(valid("read_search", { query: "x", limit })).toBeFalse();
    expect(valid("inbox_read", { limit })).toBeFalse();
  }
  for (const limit of [1, 100, 101, 1024]) expect(valid("inbox_read", { id: "PMI-1", limit })).toBeTrue();
  for (const limit of [0, 1025]) expect(valid("inbox_read", { id: "PMI-1", limit })).toBeFalse();
  expect(valid("inbox_read", { state: "bogus" })).toBeFalse();
  expect(valid("read_list", { status: "blocked" })).toBeFalse();
  expect(valid("read_list", { offset: -1 })).toBeFalse();
  expect(valid("read_show", { task: "T-1", key: "Q1" })).toBeFalse();
  expect(valid("read_show", { task: "T-1", section: "spec", key: "Q1" })).toBeFalse();
  expect(valid("read_show", { task: "T-1", section: "questions", key: "Q1", limit: 4096 })).toBeTrue();
  expect(valid("read_show", { task: "T-1", limit: 4097 })).toBeFalse();
  const claim = { id: "PMI-1", expected_revision: 0, action: "claim" };
  expect(valid("inbox_resolve", claim)).toBeTrue();
  expect(valid("inbox_resolve", { ...claim, action: "resolve" })).toBeFalse();
  expect(valid("inbox_resolve", { ...claim, action: "resolve", outcome: "done" })).toBeTrue();
  expect(valid("inbox_resolve", { ...claim, expected_revision: 4294967296 })).toBeFalse();
  expect(valid("question_cancel", { key: "Q1", rev: 4294967295, reason: "x" })).toBeTrue();
  expect(valid("question_cancel", { key: "Q1", rev: 4294967296, reason: "x" })).toBeFalse();
  expect(valid("capture", { project: "p", title: "t", body: "b", difficulty: "hard" })).toBeTrue();
});

test("required record fields, variants and collection bounds are declared", () => {
  const specs = [...pmTools, askTool, evidenceTool, checkpointTool, progressTool, waitTool,
    doneTool, verdictTool, planWriteTool("plan"), planSubmitTool, planVerdictTool];
  for (const s of specs) {
    const p = s.parameters as { required?: string[]; properties?: Record<string, unknown> };
    for (const name of p.required ?? []) {
      expect(Check(s.parameters, {})).toBeFalse();
      expect(p.properties?.[name]).toBeDefined();
    }
  }
  expect(Check(progressTool.parameters, { update: {} })).toBeFalse();
  expect(Check(checkpointTool.parameters, { checkpoint: {} })).toBeFalse();
  expect(Check(evidenceTool.parameters, { observation: {} })).toBeFalse();
  const update = { task: "T-1", run: "R1", spec_revision: "s", expected_revision: 0, phases: [
    { id: "p", label: "P", steps: [{ id: "s", label: "S", status: { state: "pending" } }] },
  ] };
  expect(Check(progressTool.parameters, { update })).toBeTrue();
  expect(Check(progressTool.parameters, { update: { ...update, expected_revision: -1 } })).toBeFalse();
  const ask = { kind: "clarification", text: "x" };
  expect(Check(askTool.parameters, { ...ask, options: Array(6).fill("x"), context_refs: Array(16).fill("x") })).toBeTrue();
  expect(Check(askTool.parameters, { ...ask, options: Array(7).fill("x") })).toBeFalse();
  expect(Check(askTool.parameters, { ...ask, context_refs: Array(17).fill("x") })).toBeFalse();
  expect(Check(receiptLookupTool().parameters, { request_id: `rq-${"a".repeat(32)}` })).toBeTrue();
  expect(Check(receiptLookupTool().parameters, { request_id: "bad" })).toBeFalse();
});

class StubClient extends CorralClient {
  calls = 0;
  response: CorralReceipt = { version: 1, request_id: toolRequestId("s", "c"), op: "read_show", state: "committed",
    identity: { project: "p" }, result: { content_json: "abc", next_offset: 3, total_chars: 10 } };
  error?: CorralCallError;
  constructor() { super({ corral: "unused", root: ".", hostCap: "unused", sessionId: () => "s",
    identity: { store: "st", project: "p", role: "pm", generation: 1 } }); }
  override async call() { this.calls++; if (this.error) throw this.error; return this.response; }
}
function fixture(s: CorralToolSpec) {
  const client = new StubClient();
  const host: ToolHost = { client, allowedOps: [s.op], sessionId: () => "s", currentTurn: () => 1 };
  const tool = defineCorralTool(s, host);
  return { client, tool, run: (args: Record<string, unknown>) => tool.execute("c", args, undefined, undefined, undefined as unknown as Parameters<typeof tool.execute>[4]) };
}

test("structured responses preserve canonical identity, codes and continuation without parsing text", async () => {
  const { client, tool, run } = fixture(spec("read_show"));
  const body = 'const text = "hello";\nconst path = "C:\\tmp"; // €\n';
  client.response = { ...client.response, result: { content_json: body, next_offset: 30, total_chars: 100 } };
  for (const state of ["committed", "rejected", "uncertain", "reserved", "queued", "abandoned"] as const) {
    client.response = { ...client.response, state };
    const result = await run({ task: "T-1" });
    expect(result.structuredContent).toEqual(client.response);
    expect(result.details).toEqual(client.response);
    const visible = result.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n");
    expect(visible).toContain(body);
    expect(visible).toContain(state);
    expect(visible).toContain(String(client.response.request_id));
    expect(visible).toContain("next_offset: 30");
    expect(Check(tool.outputSchema!, result.structuredContent)).toBeTrue();
    expect(result.isError).toBe(["rejected", "uncertain", "abandoned"].includes(state));
  }
  for (const code of ["bad_request", "stale_revision", "out_of_scope", "question_state"]) {
    client.response = { ...client.response, state: "rejected", result: { code, reason: "detail" } };
    const result = await run({ task: "T-1" });
    expect(result.structuredContent).toMatchObject({ state: "rejected", result: { code } });
  }
});

test("malformed raw calls name field and accepted shape, cause no subprocess and suggest no retry", async () => {
  const { client, tool, run } = fixture(spec("read_list"));
  const result = await run({ limit: 101 });
  expect(client.calls).toBe(0);
  expect(result.isError).toBeTrue();
  expect(Check(tool.outputSchema!, result.structuredContent)).toBeTrue();
  expect(result.structuredContent).toMatchObject({ error: { code: "bad_request", category: "invalid_arguments",
    effect: "not_started", fields: [{ path: "/limit", keyword: "maximum", accepted: { limit: 100 } }] } });
  const missing = await fixture(spec("read_search")).run({});
  expect(missing.structuredContent).toMatchObject({ error: { fields: [{ keyword: "required", accepted: { requiredProperties: ["query"] } }] } });
});

test("a timed-out done explains receipt reconciliation before retry", async () => {
  const { client, tool, run } = fixture(doneTool);
  client.error = new CorralCallError("call_timeout", "deadline", { effect: "unknown", signal: "SIGKILL", timed_out: true });
  const result = await run({ summary: "Implemented\nVerified: test\nNot verified: none" });
  expect(Check(tool.outputSchema!, result.structuredContent)).toBeTrue();
  expect(result.structuredContent).toMatchObject({ error: { category: "uncertain_effect",
    lookup: { tool: "corral_receipt_lookup", arguments: { request_id: toolRequestId("s", "c") } } } });
  expect(JSON.stringify(result.structuredContent)).toContain("If committed, completion succeeded; if no receipt exists, retry done");
});

test("transport errors distinguish no spawn from uncertain mutation and expose receipt reconciliation", async () => {
  const { client, tool, run } = fixture(spec("comment"));
  client.error = new CorralCallError("call_timeout", "deadline", { effect: "unknown", exit_code: null, signal: "SIGKILL", timed_out: true });
  let result = await run({ task: "T-1", text: "x" });
  expect(Check(tool.outputSchema!, result.structuredContent)).toBeTrue();
  expect(result.structuredContent).toMatchObject({ error: { category: "uncertain_effect", effect: "unknown", signal: "SIGKILL",
    lookup: { tool: "corral_receipt_lookup", arguments: { request_id: toolRequestId("s", "c"), task: "T-1" } } } });
  client.error = new CorralCallError("call_spawn", "missing", { effect: "not_started" });
  result = await run({ task: "T-1", text: "x" });
  expect(result.structuredContent).toMatchObject({ error: { category: "runtime_failure", effect: "not_started" } });
});
