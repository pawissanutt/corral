// Host half of gate triage: a proposal the role's authority did not cover reaches the owner once, in
// the pane's choice dialog, and only an explicit answer becomes the host-only decide call.
import { expect, test } from "bun:test";

import { TRIAGE_DECLINE, TRIAGE_LATER, TRIAGE_RECORD, askGateTriage } from "../host/triage-dialog.ts";
import { gateTriageTool } from "../roles/common/gate-triage.ts";
import { CorralCallError, type CorralCall, type CorralClient, type CorralReceipt, toolRequestId, triageDecisionRequestId } from "./corral.ts";
import { type TriageUi, defineCorralTool } from "./define.ts";

const params = { task: "T-956", diagnosis: { attempt: "G2", class: "candidate", explanation: "The parser change fails its own test.",
  evidence: [".evidence/G2/failure.log"] } };
const proposed = (request: string, extra: Record<string, unknown> = {}): CorralReceipt => ({
  version: 1, request_id: request, op: "gate_triage", state: "committed", ...extra,
  result: { outcome: "proposed", proposal: request, digest: "d".repeat(64), task: "T-956", run: "R2", head: "abc123",
    attempt: "G2", diagnosis: params.diagnosis, summary: "T-956 R2 at abc123: candidate failure G2. Records attribution only; no gate, waiver or retry.",
    confirmation_required: true },
});

/** The model-facing text of a tool result. */
function textOf(result: { content: readonly { type: string }[] }): string {
  return result.content.flatMap((part) => "text" in part && typeof part.text === "string" ? [part.text] : []).join("\n");
}

type Script = (call: CorralCall, sends: CorralCall[]) => CorralReceipt | Error;

function harness(script: Script, answer: string | undefined, hasUI = true) {
  const sends: CorralCall[] = [];
  const asked: { title: string; options: string[] }[] = [];
  const client = { call: async (_op: string, call: CorralCall) => {
    sends.push(call);
    const next = script(call, sends);
    if (next instanceof Error) throw next;
    return next;
  } } as never as CorralClient;
  const ui: TriageUi = { hasUI, ui: { select: async (title, options) => { asked.push({ title, options }); return answer; } } };
  const tool = defineCorralTool(gateTriageTool as never, {
    client, allowedOps: ["gate_triage"], sessionId: () => "s", currentTurn: () => 4,
    triage: { generation: 7, ask: (proposal, context, signal) => askGateTriage(proposal, context, signal ?? new AbortController().signal) },
  });
  return { sends, asked, ui, run: (id = "call-1") => tool.execute(id, params, undefined, undefined, ui as never) };
}

const proposing: Script = (call) => call.payload.action === "propose" ? proposed(call.requestId)
  : { version: 1, request_id: call.requestId, op: "gate_triage", state: "committed",
    result: { outcome: call.payload.approved ? "recorded" : "declined", task: "T-956", attempt: "G2" } };

test("a proposal the owner records sends one host-only decision bound to that proposal", async () => {
  const { sends, asked, run } = harness(proposing, TRIAGE_RECORD);
  const result = await run();
  const request = toolRequestId("s", "call-1");
  expect(asked).toHaveLength(1);
  expect(asked[0]?.title).toContain("Records attribution only");
  expect(asked[0]?.options).toEqual([TRIAGE_RECORD, TRIAGE_DECLINE, TRIAGE_LATER]);
  expect(sends.map((s) => s.payload.action)).toEqual(["propose", "decide"]);
  expect(sends[0]?.turn).toBe(4);
  expect(sends[1]).toEqual({ requestId: triageDecisionRequestId("s", 7, request),
    payload: { action: "decide", proposal: request, digest: "d".repeat(64), approved: true } });
  expect(sends[1]?.turn).toBeUndefined();
  expect(sends[1]?.parent).toBeUndefined();
  expect((result.details as CorralReceipt).result).toEqual({ outcome: "recorded", task: "T-956", attempt: "G2" });
  expect(result.isError).toBe(false);
});

test("an explicit decline is the only way approved:false is sent", async () => {
  const { sends, run } = harness(proposing, TRIAGE_DECLINE);
  const result = await run();
  expect(sends[1]?.payload.approved).toBe(false);
  expect((result.details as CorralReceipt).result).toMatchObject({ outcome: "declined" });
});

for (const [label, answer, hasUI] of [
  ["cancelled or timed out", undefined, true], ["decide later", TRIAGE_LATER, true], ["no dialog UI", TRIAGE_RECORD, false],
] as const) test(`${label} leaves the proposal pending: nothing is decided and the result is not an error`, async () => {
  const { sends, asked, run } = harness(proposing, answer, hasUI);
  const result = await run();
  expect(sends.map((s) => s.payload.action)).toEqual(["propose"]);
  expect(asked).toHaveLength(hasUI ? 1 : 0);
  expect((result.details as CorralReceipt).result).toMatchObject({ outcome: "proposed", confirmation_required: true });
  expect(result.isError).toBe(false);
});

test("a diagnosis the role's authority already recorded asks nobody and decides nothing", async () => {
  const { sends, asked, run } = harness((call) => ({ version: 1, request_id: call.requestId, op: "gate_triage", state: "committed",
    result: { outcome: "recorded", task: "T-956", attempt: "G2" } }), TRIAGE_RECORD);
  await run();
  expect([sends.length, asked.length]).toEqual([1, 0]);
});

test("a replayed proposal after a resend or reconnect is not shown again", async () => {
  const { sends, asked, run } = harness((call) => proposed(call.requestId, { replayed: true }), TRIAGE_RECORD);
  const result = await run();
  expect([sends.length, asked.length]).toEqual([1, 0]);
  expect((result.details as CorralReceipt).result).toMatchObject({ outcome: "proposed" });
});

test("a refused decision (stale, held or changed scope) is returned with its reason and never retried", async () => {
  const { sends, asked, run } = harness((call) => call.payload.action === "propose" ? proposed(call.requestId)
    : { version: 1, request_id: call.requestId, op: "gate_triage", state: "rejected",
      result: { code: "stale", reason: "candidate changed since the proposal" } }, TRIAGE_RECORD);
  const result = await run();
  expect(result.isError).toBe(true);
  expect(textOf(result)).toContain("candidate changed since the proposal");
  expect([sends.length, asked.length]).toEqual([2, 1]);
});

test("an uncertain decision names its own request for lookup, without a second prompt or decide", async () => {
  const lost = new CorralCallError("call_timeout", "lost the decision", { effect: "unknown" });
  const { sends, asked, run } = harness((call) => call.payload.action === "propose" ? proposed(call.requestId) : lost, TRIAGE_RECORD);
  const result = await run();
  const decision = triageDecisionRequestId("s", 7, toolRequestId("s", "call-1"));
  expect(result.isError).toBe(true);
  expect(result.details).toMatchObject({ request_id: decision, error: { category: "uncertain_effect",
    lookup: { tool: "corral_receipt_lookup", arguments: { request_id: decision } } } });
  expect(textOf(result)).toContain("Do not propose again");
  expect([sends.length, asked.length]).toEqual([2, 1]);
});

test("a dialog the turn aborted, or planning has fenced, is not the owner's answer", async () => {
  const proposal = { proposal: "rq-p", digest: "d", summary: "s" };
  const aborted = new AbortController();
  const ui: TriageUi = { hasUI: true, ui: { select: async () => { aborted.abort(); return TRIAGE_RECORD; } } };
  expect(await askGateTriage(proposal, ui, aborted.signal)).toBeUndefined();
  const open: TriageUi = { hasUI: true, ui: { select: async () => TRIAGE_RECORD } };
  expect(await askGateTriage(proposal, open, new AbortController().signal, () => "assistant_planning: fenced")).toBeUndefined();
});
