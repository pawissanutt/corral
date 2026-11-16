// Turns a role's CorralToolSpec into an upstream tool whose every call is one corral
// request with a request ID derived from the session and the model's tool call ID, or, for a
// call a Code Mode cell made, from the child's parent-bound key (T-683).
import type { ExtensionUIContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Errors } from "typebox/value";
import type { JsonValue } from "@earendil-works/pi-ai";
import { corralOutput } from "./results.ts";
import { renderFailure, renderReceipt } from "./presentation.ts";

import type { CorralOp, CorralToolSpec } from "../roles/types.ts";
import { nestedChild } from "../extensions/codemode/child.ts";
import { nestedCorralRead } from "../roles/common/execution.ts";
import { CorralCallError, type CorralClient, type CorralReceipt, toolRequestId, triageDecisionRequestId } from "./corral.ts";
import type { FleetSnapshot } from "../build/types.ts";
import { finishHereEvidence } from "../roles/assistant/owner-action.ts";

/** The pane UI a confirmation is shown in; absent when the host has no dialog-capable UI. */
export interface ConfirmUi {
  readonly hasUI: boolean;
  readonly ui: Pick<ExtensionUIContext, "confirm">;
}

/** The pane UI a gate-triage proposal is shown in, with a distinct cancel (`select` answers undefined). */
export interface TriageUi {
  readonly hasUI: boolean;
  readonly ui: Pick<ExtensionUIContext, "select">;
}

export interface ToolHost {
  readonly client: CorralClient;
  readonly allowedOps: readonly CorralOp[];
  sessionId(): string;
  currentTurn(): number | undefined;
  fleetSnapshot?(): FleetSnapshot | undefined;
  /**
   * Shows the owner `command` and journals the answer bound to `requestId`; true once the
   * answer is journaled. Only an assistant host provides it.
   */
  confirm?(command: string, context: ConfirmUi, requestId: string): Promise<boolean>;
  /**
   * Gate triage: the host's own half of a `proposed` result. `ask` shows the owner one proposal in
   * the pane's choice dialog: true to record it, false for an explicit decline, undefined when
   * nobody decided (no dialog UI, cancelled, timed out, aborted, or "decide later"). The PM and
   * assistant hosts provide it; the model never can.
   */
  readonly triage?: {
    readonly generation: number;
    ask(proposal: GateTriageProposal, context: TriageUi, signal: AbortSignal | undefined): Promise<boolean | undefined>;
  };
}

export function corralToolNames(specs: readonly CorralToolSpec[]): string[] {
  return specs.map((spec) => spec.name);
}

/** The command a `confirmation_required` refusal asks the owner to confirm. */
export function confirmationCommand(receipt: CorralReceipt): string | undefined {
  const result = receipt.result as { code?: unknown; command?: unknown } | undefined;
  return receipt.state === "rejected" && result?.code === "confirmation_required" && typeof result.command === "string"
    ? result.command : undefined;
}

/** What the owner is asked to approve: the stored proposal, its digest and corral's own summary of it. */
export interface GateTriageProposal {
  readonly proposal: string;
  readonly digest: string;
  readonly summary: string;
}

/** The proposal a committed `gate_triage` receipt is waiting on the owner for, if it is one. */
export function gateTriageProposal(receipt: CorralReceipt): GateTriageProposal | undefined {
  const result = receipt.result as { outcome?: unknown; proposal?: unknown; digest?: unknown;
    summary?: unknown; confirmation_required?: unknown } | null | undefined;
  if (receipt.state !== "committed" || result?.outcome !== "proposed" || result.confirmation_required !== true) return undefined;
  const { proposal, digest, summary } = result;
  return typeof proposal === "string" && proposal !== "" && typeof digest === "string" && digest !== ""
    && typeof summary === "string" && summary !== "" ? { proposal, digest, summary } : undefined;
}

export function defineCorralTool(spec: CorralToolSpec, host: ToolHost): ToolDefinition {
  if (!host.allowedOps.includes(spec.op)) {
    throw new Error(`role tool ${spec.name} calls ${spec.op}, which the role does not allow`);
  }
  return {
    name: spec.name,
    label: spec.label,
    description: spec.description,
    exposure: "direct",
    parameters: spec.parameters,
    outputSchema: corralOutput(spec.resultSchema),
    async execute(toolCallId, params, _signal, _update, context) {
      const child = nestedChild(spec.name, toolCallId);
      const requestId = toolRequestId(host.sessionId(), child?.key ?? toolCallId);
      const failure = (error: Record<string, unknown>, id = requestId) => {
        const value = { request_id: id, op: spec.op, error };
        return { content: [{ type: "text" as const, text: renderFailure(spec.op, id, error) }], details: value,
          structuredContent: value as JsonValue, isError: true };
      };
      // Pi validates normal calls as well. Keep raw execute()/host callers safe and return
      // field paths plus accepted constraints without echoing potentially secret input.
      const fields = Errors(spec.parameters, params).map((e) => ({
        path: e.instancePath, keyword: e.keyword, accepted: e.params,
      }));
      if (fields.length) return failure({ code: "bad_request", category: "invalid_arguments",
        reason: "Arguments do not match the tool schema", effect: "not_started", fields });
      let payload: Record<string, unknown>;
      try { payload = spec.payload(params as Record<string, unknown>); }
      catch (error) {
        return failure({ code: "bad_payload", category: "invalid_arguments", effect: "not_started",
          reason: error instanceof Error ? error.message : "Invalid payload" });
      }
      if (spec.name === "corral_owner_action" && payload.kind === "finish_here") {
        payload = { ...payload, ...finishHereEvidence(host.fleetSnapshot?.()) };
      }
      const send = (body: Record<string, unknown>) => host.client.call(spec.op, {
        requestId, payload: body, turn: host.currentTurn(), ...(child ? { parent: child.parent } : {}),
        ...(spec.timeoutMs !== undefined ? { timeoutMs: spec.timeoutMs } : {}),
      });
      let receipt: CorralReceipt;
      // The request a transport failure must name: the call, or the host's own decision on its proposal.
      let sent = { id: requestId, decision: false };
      try {
        receipt = await send(payload);
        // A confirmation refusal stores nothing; repeat only after its bound owner answer.
        const command = confirmationCommand(receipt);
        const answered = command !== undefined && context !== undefined
          && await host.confirm?.(command, context, requestId) === true;
        if (answered) receipt = await send(payload);
        // A gate-triage proposal is shown to the owner once, on the call that created it. A replayed
        // receipt is a resend or a reconnect: it never asks again, and the proposal stays proposed.
        const proposal = spec.op === "gate_triage" && context !== undefined && receipt.replayed !== true
          ? gateTriageProposal(receipt) : undefined;
        const approved = proposal && context !== undefined
          ? await host.triage?.ask(proposal, context, _signal) : undefined;
        if (proposal && host.triage && approved !== undefined) {
          // Host-only: no turn and no parent, which corral requires of a decision.
          const decision = { action: "decide", proposal: proposal.proposal, digest: proposal.digest, approved };
          sent = { id: triageDecisionRequestId(host.sessionId(), host.triage.generation, proposal.proposal), decision: true };
          receipt = await host.client.call(spec.op, {
            requestId: sent.id, payload: decision,
            ...(spec.timeoutMs !== undefined ? { timeoutMs: spec.timeoutMs } : {}),
          });
        }
      } catch (error) {
        if (!(error instanceof CorralCallError)) throw error;
        const effect = error.diagnostics.effect ?? (error.code === "bad_payload" ? "not_started" : "unknown");
        return failure({ code: error.code,
          category: error.code === "bad_payload" ? "invalid_arguments"
            : effect === "unknown" && !nestedCorralRead(spec.op) ? "uncertain_effect" : "runtime_failure",
          reason: error.code === "call_timeout" && spec.op === "done"
            ? `${error.message}. The executor was stopped; look up this request's receipt. If committed, completion succeeded; if no receipt exists, retry done. Check task status before retrying.`
            : sent.decision
              ? `${error.message}. The owner answered the proposal, but whether corral recorded that answer is unknown: look up this decision request's receipt. Do not propose again or ask the owner again.`
              : error.message, ...error.diagnostics, effect,
          ...(effect === "unknown" && !nestedCorralRead(spec.op) ? { lookup: { tool: "corral_receipt_lookup", arguments: {
            request_id: sent.id,
            ...(["comment", "reply"].includes(spec.op) && typeof payload.task === "string" ? { task: payload.task } : {}),
            ...(["question_answer", "question_escalate", "question_cancel"].includes(spec.op)
              && typeof payload.key === "string" ? { task: payload.key.split("/")[0] } : {}),
          } } } : {}),
        }, sent.id);
      }
      return {
        content: [{ type: "text", text: renderReceipt(receipt) }],
        details: receipt,
        structuredContent: receipt as JsonValue,
        isError: receipt.state === "rejected" || receipt.state === "uncertain" || receipt.state === "abandoned",
        // These committed operations close the review or run; another model turn cannot reserve budget.
        ...((spec.op === "verdict" || spec.op === "plan_verdict" || spec.op === "done") && receipt.state === "committed"
          ? { terminate: true } : {}),
      };
    },
  };
}
