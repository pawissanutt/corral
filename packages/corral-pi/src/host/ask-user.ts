import type { ExtensionToolContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { OWNER_INPUT_TEXT_MAX } from "../roles/assistant/owner-action.ts";
import { AskDialog, type Choice, type DialogAnswer, type DialogQuestion } from "./ask-user-dialog.ts";

const choice = Type.Union([
  Type.String({ minLength: 1, maxLength: 300 }),
  Type.Object({
    label: Type.String({ minLength: 1, maxLength: 300 }),
    description: Type.Optional(Type.String({ maxLength: 500, description: "One line explaining the option" })),
  }),
]);

const question = Type.Object({
  question: Type.String({ minLength: 1, maxLength: 1000 }),
  header: Type.Optional(Type.String({ minLength: 1, maxLength: 24, description: "Short tab label, for example \"Scope\"" })),
  options: Type.Array(choice, { minItems: 2, maxItems: 4 }),
  multiSelect: Type.Optional(Type.Boolean({ description: "Allow choosing several options" })),
  freeText: Type.Optional(Type.Boolean({ description: "Offer an optional text field after the choice" })),
});

type Question = { question: string; header?: string; options: (string | Choice)[]; multiSelect?: boolean; freeText?: boolean };
type Answer = { question: string; option: string; options?: string[]; text?: string };

const TIMEOUT_MS = 300_000;

export function askUserEnabled(role: string, enabled: readonly string[]): boolean {
  return enabled.includes("ask_user") && (role === "assistant" || role === "pm");
}

function normalize(q: Question): DialogQuestion {
  const options = q.options.map((o) => (typeof o === "string" ? { label: o } : o));
  if (new Set(options.map((o) => o.label)).size !== options.length) throw new Error("Options must be distinct");
  return { ...q, options };
}

/** One dialog for the whole batch; Escape, abort or timeout keeps what was answered. Undefined when the
 * host shows no custom component (a select-only or no-op UI context, or RPC mode). */
async function dialog(ctx: ExtensionToolContext, questions: DialogQuestion[], signal?: AbortSignal) {
  let current: AskDialog | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const stop = () => current?.finish(false);
  signal?.addEventListener("abort", stop, { once: true });
  try {
    const result = await ctx.ui.custom<{ submitted: boolean; answers: (DialogAnswer | undefined)[] } | undefined>((tui, _theme, _keys, done) => {
      const arm = () => { clearTimeout(timer); timer = setTimeout(stop, TIMEOUT_MS); };
      current = new AskDialog(questions, done, () => { arm(); tui.requestRender(); });
      arm();
      return current;
    }, { blocked: { kind: "question", message: "Waiting for your answers" } });
    return result?.answers;
  } catch (error) {
    // The host wraps every UI context with a `custom` that forwards to the context's own; one without
    // a component throws here before showing anything, so the caller falls back to plain selects.
    if (current === undefined) return undefined;
    throw error;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", stop);
  }
}

/** Hosts without a custom component ask one question after another; a cancel keeps earlier answers. */
async function sequential(ctx: ExtensionToolContext, questions: DialogQuestion[], signal?: AbortSignal) {
  const answers: (DialogAnswer | undefined)[] = questions.map(() => undefined);
  for (const [i, q] of questions.entries()) {
    const labels = q.options.map((o) => o.label);
    const option = await ctx.ui.select(q.question, labels, { signal, timeout: TIMEOUT_MS });
    if (option === undefined || !labels.includes(option) || signal?.aborted) break;
    let text = "";
    if (q.freeText) {
      const typed = await ctx.ui.input("Optional additional text (leave empty to skip)", "", { signal, timeout: TIMEOUT_MS });
      if (typed === undefined || signal?.aborted) break;
      text = typed;
    }
    answers[i] = { selected: [option], text };
  }
  return answers;
}

/** Dialogs are local to the owner's interactive pane, never a worker question flow. */
export function askUserTool(record: (answer: string) => Promise<string>): ToolDefinition {
  return {
    name: "ask_user", label: "Ask the owner", exposure: "model-only", executionMode: "sequential",
    description: "Ask the owner 1-4 focused choice questions together in one dialog; batch related questions in "
      + "one call. Each takes a short header, 2-4 options (string or {label, description}), optional multiSelect "
      + "and freeText. The owner may answer only some: unanswered ones are listed. Cancellation or timeout is not consent.",
    parameters: Type.Object({ questions: Type.Array(question, { minItems: 1, maxItems: 4 }) }),
    async execute(_id, params: { questions: Question[] }, signal, _update, ctx) {
      const noAnswer = () => ({ content: [{ type: "text" as const, text: "no answer" }], details: { status: "no answer" } });
      const questions = params.questions.map(normalize);
      if (!ctx.hasUI || signal?.aborted) return noAnswer();
      const shown = typeof ctx.ui.custom === "function" ? await dialog(ctx, questions, signal) : undefined;
      if (signal?.aborted && shown === undefined) return noAnswer();
      const raw = shown ?? await sequential(ctx, questions, signal);
      const answers: Answer[] = [];
      const unanswered: string[] = [];
      raw.forEach((a, i) => {
        const q = questions[i]!;
        if (!a || a.selected.length === 0) { unanswered.push(q.question); return; }
        answers.push({
          question: q.question, option: a.selected.join(", "),
          ...(q.multiSelect ? { options: a.selected } : {}), ...(a.text ? { text: a.text } : {}),
        });
      });
      if (answers.length === 0) return noAnswer();
      const lines = answers.map((a) => `${a.question}: ${a.option}${a.text ? `\n${a.text}` : ""}`);
      if (unanswered.length) lines.push(`Unanswered (no consent): ${unanswered.join(" | ")}`);
      const answer = lines.join("\n");
      // The cited owner instruction must be stored in full, not as a truncated prefix.
      if (Buffer.byteLength(answer) > OWNER_INPUT_TEXT_MAX) return noAnswer();
      const recorded = await record(answer);
      const status = unanswered.length ? "partial" : "answered";
      return { content: [{ type: "text", text: recorded }], details: { status, answers, ...(unanswered.length ? { unanswered } : {}) } };
    },
  };
}
