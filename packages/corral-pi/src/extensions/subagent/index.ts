import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { StringEnum } from "@earendil-works/pi-ai";
import type { AuxiliaryRuntime } from "./runtime.ts";
import type { ResearchOptions } from "./research.ts";

const researchSchema = Type.Object({
  prompt: Type.String({ minLength: 1, maxLength: 32768, description: "One bounded question and the evidence/output needed. Children do not inherit the conversation." }),
  profile: Type.Optional(StringEnum(["code", "docs"] as const)),
  effort: Type.Optional(StringEnum(["quick", "standard", "deep"] as const)),
  scope: Type.Optional(Type.String({ maxLength: 2048, description: "Research scope; advisory, not an additional access boundary" })),
  starting_points: Type.Optional(Type.String({ maxLength: 4096, description: "Relevant files, symbols or official documentation URLs" })),
  constraints: Type.Optional(Type.String({ maxLength: 4096, description: "Relevant project rules and requirements; do not paste the whole conversation" })),
});

export function exploreTool(runtime: AuxiliaryRuntime): ToolDefinition {
  return {
    name: "explore", label: "Explore with a read-only child",
    description: "Delegate a bounded research question to a read-only code or documentation helper. "
      + "Supply scope, starting points and relevant constraints; children do not inherit your conversation. "
      + "Use quick for a narrow scan (at most four turns), standard for tracing, deep for harder reasoning. "
      + "All use the configured child model and stay within its turn/time limits; no writes, shell, MCP or recursion. "
      + "Concurrent calls beyond the configured cap queue. Returns status, stop reason, request/transcript references and findings capped at 32,768 characters.",
    promptSnippet: "Delegate independent code or documentation research with cited, bounded findings",
    promptGuidelines: [
      "Use explore for substantial independent research that would otherwise fill your context with search output, not trivial one-file lookups.",
      "For broad research, split explore into two or three non-overlapping questions and run them concurrently (codemode Promise.all when available). Keep useful separate work for yourself; do not duplicate child searches while waiting.",
      "Give explore a question, relevant constraints, starting points and expected evidence. Select profile docs for official/version-specific documentation, code for execution paths. Do not assume children inherit your conversation or project instructions.",
      "Collect and reconcile explore findings before claiming completion. Check partial/failed status and coverage; verify important citations. Research helpers are not independent Corral reviewers and grant no board authority.",
    ],
    parameters: researchSchema,
    async execute(id, params: { prompt: string } & ResearchOptions, signal) {
      const { prompt, ...research } = params;
      const size = [prompt, research.scope, research.starting_points, research.constraints].reduce((n, text) => n + (text?.length ?? 0), 0);
      if (size > 32768) throw new Error("Research question and context must total at most 32,768 characters");
      const result = await runtime.run("explore", id, prompt, signal, research);
      const status = result.report?.status ?? (result.outcome === "completed" ? "completed" : "partial");
      const header = [`Research ${status}; stop: ${result.report?.stopReason ?? result.outcome}.`,
        `Request: ${result.request}; generation: ${result.generation}.`,
        ...(result.sessionFile ? [`Transcript: ${result.sessionFile}`] : []),
        ...(result.truncated ? ["Findings truncated to 32,768 characters; inspect the transcript for the full answer."] : []),
      ].join("\n");
      return { content: [{ type: "text", text: `${header}\n\n${result.text}` }], details: result, isError: status === "failed" };
    },
  };
}
