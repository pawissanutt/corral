// Turns a role's CorralToolSpec into an upstream tool whose every call is one corral
// request with a request ID derived from the session and the model's tool call ID.
import type { ExtensionUIContext, ToolDefinition } from "@earendil-works/pi-coding-agent";

import type { CorralOp, CorralToolSpec } from "../roles/types.ts";
import { type CorralClient, type CorralReceipt, toolRequestId } from "./corral.ts";

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
   * Shows the owner `command` and journals the answer, returning the answer's owner-input ID;
   * undefined when no answer could be journaled. Only an assistant host provides it.
   */
  confirm?(command: string, context: ConfirmUi): Promise<string | undefined>;
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
    description: spec.description,
    parameters: spec.parameters,
    async execute(toolCallId, params, _signal, _update, context) {
      const requestId = toolRequestId(host.sessionId(), toolCallId);
      const payload = spec.payload(params as Record<string, unknown>);
      const send = (body: Record<string, unknown>) => host.client.call(spec.op, {
        requestId, payload: body, turn: host.currentTurn(),
        ...(spec.timeoutMs !== undefined ? { timeoutMs: spec.timeoutMs } : {}),
      });
      let receipt = await send(payload);
      // A refusal stores nothing, so the same request carries the owner's journaled answer.
      const command = confirmationCommand(receipt);
      const answer = command !== undefined && context ? await host.confirm?.(command, context) : undefined;
      if (answer !== undefined) receipt = await send({ ...payload, confirmation: answer });
      return {
        content: [{ type: "text", text: JSON.stringify({ state: receipt.state, result: receipt.result }) }],
        details: receipt,
      };
    },
  };
}
