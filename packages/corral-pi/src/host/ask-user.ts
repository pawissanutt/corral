import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { OWNER_INPUT_TEXT_MAX } from "../roles/assistant/owner-action.ts";

const question = Type.Object({
  question: Type.String({ minLength: 1, maxLength: 1000 }),
  options: Type.Array(Type.String({ minLength: 1, maxLength: 300 }), { minItems: 2, maxItems: 4 }),
  freeText: Type.Optional(Type.Boolean({ description: "Offer an optional text field after the choice" })),
});

type Question = { question: string; options: string[]; freeText?: boolean };

export function askUserEnabled(role: string, enabled: readonly string[]): boolean {
  return enabled.includes("ask_user") && (role === "assistant" || role === "pm");
}

/** Dialogs are local to the owner's interactive pane, never a worker question flow. */
export function askUserTool(record: (answer: string) => Promise<string>): ToolDefinition {
  return {
    name: "ask_user", label: "Ask the owner", exposure: "model-only", executionMode: "sequential",
    description: "Ask the owner 1-4 focused choice questions. Cancellation or timeout is not consent.",
    parameters: Type.Object({ questions: Type.Array(question, { minItems: 1, maxItems: 4 }) }),
    async execute(_id, params: { questions: Question[] }, signal, _update, ctx) {
      const noAnswer = () => ({ content: [{ type: "text" as const, text: "no answer" }], details: { status: "no answer" } });
      if (!ctx.hasUI || signal?.aborted) return noAnswer();
      const answers: { question: string; option: string; text?: string }[] = [];
      for (const q of params.questions) {
        if (new Set(q.options).size !== q.options.length) throw new Error("Options must be distinct");
        const option = await ctx.ui.select(q.question, q.options, { signal, timeout: 300_000 });
        if (option === undefined || !q.options.includes(option) || signal?.aborted) return noAnswer();
        let text: string | undefined;
        if (q.freeText) {
          text = await ctx.ui.input("Optional additional text (leave empty to skip)", "", { signal, timeout: 300_000 });
          if (text === undefined || signal?.aborted) return noAnswer();
        }
        answers.push({ question: q.question, option, ...(text ? { text } : {}) });
      }
      const answer = answers.map((a) => `${a.question}: ${a.option}${a.text ? `\n${a.text}` : ""}`).join("\n");
      // The cited owner instruction must be stored in full, not as a truncated prefix.
      if (Buffer.byteLength(answer) > OWNER_INPUT_TEXT_MAX) return noAnswer();
      const recorded = await record(answer);
      return { content: [{ type: "text", text: recorded }], details: { status: "answered", answers } };
    },
  };
}
