// Turns a role's CorralToolSpec into an upstream tool whose every call is one corral
// request with a request ID derived from the session and the model's tool call ID.
import type { ExtensionUIContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Errors } from "typebox/value";
import type { JsonValue } from "@earendil-works/pi-ai";
import { corralOutput } from "./results.ts";

import type { CorralOp, CorralToolSpec } from "../roles/types.ts";
import { nestedCorralRead } from "../roles/common/execution.ts";
import { CorralCallError, type CorralClient, type CorralReceipt, toolRequestId } from "./corral.ts";

/** The pane UI a confirmation is shown in; absent when the host has no dialog-capable UI. */
export interface ConfirmUi {
  readonly hasUI: boolean;
  readonly ui: Pick<ExtensionUIContext, "confirm">;
}

export interface ToolHost {
  readonly client: CorralClient;
  readonly allowedOps: readonly CorralOp[];
  sessionId(): string;
  currentTurn(): number | undefined;
  /**
   * Shows the owner `command` and journals the answer bound to `requestId`; true once the
   * answer is journaled. Only an assistant host provides it.
   */
  confirm?(command: string, context: ConfirmUi, requestId: string): Promise<boolean>;
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

export function defineCorralTool(spec: CorralToolSpec, host: ToolHost): ToolDefinition {
  if (!host.allowedOps.includes(spec.op)) {
    throw new Error(`role tool ${spec.name} calls ${spec.op}, which the role does not allow`);
  }
  return {
    name: spec.name,
    label: spec.label,
    description: spec.description + (nestedCorralRead(spec.op) ? ""
      : " Call directly at top level, not from Code Mode or another tool: receipt replay/recovery requires a transcript call."),
    exposure: nestedCorralRead(spec.op) ? "direct" : "model-only",
    parameters: spec.parameters,
    outputSchema: corralOutput(spec.resultSchema),
    async execute(toolCallId, params, _signal, _update, context) {
      const requestId = toolRequestId(host.sessionId(), toolCallId);
      const failure = (error: Record<string, unknown>) => {
        const value = { request_id: requestId, op: spec.op, error };
        return { content: [{ type: "text" as const, text: JSON.stringify(value) }], details: value,
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
      const send = (body: Record<string, unknown>) => host.client.call(spec.op, {
        requestId, payload: body, turn: host.currentTurn(),
        ...(spec.timeoutMs !== undefined ? { timeoutMs: spec.timeoutMs } : {}),
      });
      let receipt: CorralReceipt;
      try {
        receipt = await send(payload);
        // A confirmation refusal stores nothing; repeat only after its bound owner answer.
        const command = confirmationCommand(receipt);
        const answered = command !== undefined && context !== undefined
          && await host.confirm?.(command, context, requestId) === true;
        if (answered) receipt = await send(payload);
      } catch (error) {
        if (!(error instanceof CorralCallError)) throw error;
        const effect = error.diagnostics.effect ?? (error.code === "bad_payload" ? "not_started" : "unknown");
        return failure({ code: error.code,
          category: error.code === "bad_payload" ? "invalid_arguments"
            : effect === "unknown" && !nestedCorralRead(spec.op) ? "uncertain_effect" : "runtime_failure",
          reason: error.message, ...error.diagnostics, effect,
          ...(effect === "unknown" && !nestedCorralRead(spec.op) ? { lookup: { tool: "corral_receipt_lookup", arguments: {
            request_id: requestId,
            ...(["comment", "reply"].includes(spec.op) && typeof payload.task === "string" ? { task: payload.task } : {}),
            ...(["question_answer", "question_escalate", "question_cancel"].includes(spec.op)
              && typeof payload.key === "string" ? { task: payload.key.split("/")[0] } : {}),
          } } } : {}),
        });
      }
      return {
        content: [{ type: "text", text: JSON.stringify(receipt) }],
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
