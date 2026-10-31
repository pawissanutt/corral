import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { AuxiliaryRuntime } from "./runtime.ts";

export function exploreTool(runtime: AuxiliaryRuntime): ToolDefinition {
  return {
    name: "explore", label: "Explore with a read-only child",
    description: "Ask a read-only child to research the worktree. Children cannot write, run shell commands or recurse. "
      + "Concurrent calls beyond the configured cap queue until a child finishes. Results and usage belong to this parent generation.",
    parameters: Type.Object({ prompt: Type.String({ minLength: 1, maxLength: 32768 }) }),
    async execute(id, params: { prompt: string }, signal) {
      const result = await runtime.run("explore", id, params.prompt, signal);
      return { content: [{ type: "text", text: result.text }], details: result };
    },
  };
}
