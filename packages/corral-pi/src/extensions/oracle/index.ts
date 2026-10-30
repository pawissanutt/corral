import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { AuxiliaryRuntime } from "../subagent/runtime.ts";

export function oracleTool(runtime: AuxiliaryRuntime): ToolDefinition {
  return {
    name: "oracle", label: "Consult a configured model",
    description: "Consult the configured oracle model once, with no tools. Include all necessary context in the prompt. "
      + "Uses the bound provider credentials, records auxiliary usage, and preserves uncertain spend on cancellation. "
      + "Requires agent.capabilities.<role>.oracle.model; it never reads another harness's credentials.",
    parameters: Type.Object({ prompt: Type.String({ minLength: 1, maxLength: 32768 }) }),
    async execute(id, params: { prompt: string }, signal) {
      const result = await runtime.run("oracle", id, params.prompt, signal);
      return { content: [{ type: "text", text: result.text }], details: result };
    },
  };
}
