// Turns a role's CorralToolSpec into an upstream tool whose every call is one corral
// request with a request ID derived from the session and the model's tool call ID.
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";

import type { CorralOp, CorralToolSpec } from "../roles/types.ts";
import { type CorralClient, toolRequestId } from "./corral.ts";

export interface ToolHost {
  readonly client: CorralClient;
  readonly allowedOps: readonly CorralOp[];
  sessionId(): string;
  currentTurn(): number | undefined;
}

export function corralToolNames(specs: readonly CorralToolSpec[]): string[] {
  return specs.map((spec) => spec.name);
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
    async execute(toolCallId, params) {
      const receipt = await host.client.call(spec.op, {
        requestId: toolRequestId(host.sessionId(), toolCallId),
        payload: spec.payload(params as Record<string, unknown>),
        turn: host.currentTurn(),
      });
      return {
        content: [{ type: "text", text: JSON.stringify({ state: receipt.state, result: receipt.result }) }],
        details: receipt,
      };
    },
  };
}
