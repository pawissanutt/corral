import { expect, test } from "bun:test";
import { renderBashResult, renderJobResult } from "../src/extensions/jobs/presentation.ts";
import { renderReceipt } from "../src/tools/presentation.ts";
import type { CorralReceipt } from "../src/tools/corral.ts";
import { buildPromptBlocks } from "../src/claude/prompt.ts";

test("readable results retain recovery metadata, unfamiliar data and canonical state without escaping bodies", () => {
  const body = 'const path = "C:\\tmp";\n// € and \\"quotes\\"\n';
  const job = { job: "j1-1", state: "exited", exit_code: null, signal: "SIGTERM", truncated: true,
    offset: 12, cursor: "j1-1:40", has_more: true, gap: { from: 0, to: 12 }, text: body, future: { code: "retained" } };
  const snapshot = JSON.stringify(job);
  for (const visible of [renderJobResult(job), renderBashResult(job)]) {
    expect(visible).toContain(body);
    for (const field of ["SIGTERM", "truncated: true", "j1-1:40", "has_more: true", 'gap: {"from":0,"to":12}', "retained"]) {
      expect(visible).toContain(field);
    }
  }
  expect(JSON.stringify(job)).toBe(snapshot);
  expect(renderJobResult({ state: "lost", reason: "outcome unknown; command was not replayed" })).toContain("outcome unknown");

  const receipt = { version: 1, request_id: "rq-example", op: "todo", state: "committed", replayed: true,
    identity: { store: "store", role: "assistant" }, payload_sha256: "digest", effect: { kind: "progress", ref: "4" },
    result: { plan: "P1", revision: 4, disposition: { state: "waiting_owner", steps: ["fix"] }, progress: {
      plan: "P1", revision: 4, spec_revision: "assistant:A-1", phases: [{ id: "p", label: "Implement", steps: [
        { id: "fix", label: "Fix the output", criteria: [], status: { state: "in_progress" }, future: body },
      ] }], waits: [{ step: "fix", on: { kind: "owner", question: "ship?" } }],
    }, future: { next: "retain me" } }, future_envelope: { data: "retain this too" } } satisfies CorralReceipt;
  const original = JSON.stringify(receipt);
  const visible = renderReceipt(receipt);
  for (const field of ["todo: committed (replayed)", "rq-example", "revision: 4", "fix [in_progress]", "ship?", body,
    "retain me", "retain this too", 'effect: {"kind":"progress","ref":"4"}']) expect(visible).toContain(field);
  expect(visible.includes("payload_sha256")).toBeFalse();
  expect(JSON.stringify(receipt)).toBe(original);
  const prompt = buildPromptBlocks([{ role: "toolResult", toolName: "corral_todo", toolCallId: "call",
    content: [{ type: "text", text: visible }], details: receipt, isError: false, timestamp: 0 }], new Map());
  const modelText = prompt.flatMap(part => part.type === "text" ? [part.text] : []).join("\n");
  expect(modelText).toContain(visible);
  expect(modelText.includes("payload_sha256")).toBeFalse();
  // Future protocols/operations fall back to the entire canonical response, not an older summary.
  for (const unknown of [{ ...receipt, version: 2 }, { ...receipt, op: "future_operation" }]) {
    expect(JSON.parse(renderReceipt(unknown))).toEqual(unknown);
  }
});
