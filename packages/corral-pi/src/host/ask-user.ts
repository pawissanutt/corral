import type { ExtensionToolContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { OWNER_INPUT_TEXT_MAX } from "../roles/assistant/owner-action.ts";
import { ASK_USER_GUIDELINES } from "../roles/common/ask-user.ts";
import { AskDialog, type Choice, type DialogAnswer, type DialogOptions, type DialogQuestion, type DialogResult } from "./ask-user-dialog.ts";
import type { ChoiceQuestion, PendingQuestions } from "./pending-questions.ts";

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
  freeText: Type.Optional(Type.Boolean({ description: "Owners can always type an Other answer unless false; true also allows a note on the chosen option" })),
});

const deferredQuestion = Type.Object({
  question: Type.String({ minLength: 1, maxLength: 1000 }),
  options: Type.Array(Type.String({ minLength: 1, maxLength: 300 }), { minItems: 2, maxItems: 4 }),
  freeText: Type.Optional(Type.Boolean()),
}, { additionalProperties: false });

type Question = { question: string; header?: string; options: (string | Choice)[]; multiSelect?: boolean; freeText?: boolean };
type Answer = { question: string; option?: string; options?: string[]; text?: string };

const TIMEOUT_MS = 300_000;

export function askUserEnabled(role: string, enabled: readonly string[]): boolean {
  return enabled.includes("ask_user") && (role === "assistant" || role === "pm");
}

function normalize(q: Question): DialogQuestion {
  const options = q.options.map((o) => (typeof o === "string" ? { label: o } : o));
  if (new Set(options.map((o) => o.label)).size !== options.length) throw new Error("Options must be distinct");
  return { ...q, options };
}

type DialogUi = Pick<ExtensionToolContext["ui"], "custom" | "select" | "input">;

/** One dialog for the whole batch; Escape, abort or timeout keeps what was answered. Undefined when the
 * host shows no custom component (a select-only or no-op UI context, or RPC mode). */
export async function askDialog(ui: DialogUi, questions: DialogQuestion[], signal?: AbortSignal,
  options?: DialogOptions): Promise<DialogResult | undefined> {
  if (typeof ui.custom !== "function") return undefined;
  let current: AskDialog | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const stop = () => current?.finish(false);
  signal?.addEventListener("abort", stop, { once: true });
  try {
    const result = await ui.custom<DialogResult | undefined>((tui, _theme, _keys, done) => {
      const arm = () => { clearTimeout(timer); timer = setTimeout(stop, TIMEOUT_MS); };
      current = new AskDialog(questions, done, () => { arm(); tui.requestRender(); }, options);
      arm();
      if (signal?.aborted) stop();
      return current;
    }, { blocked: { kind: "question", message: "Waiting for your answers" } });
    return result ?? { submitted: false, answers: questions.map(() => undefined) };
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

const OTHER = "Other (type an answer)";

/** Hosts without a custom component ask one question after another; a cancel keeps earlier answers.
 * Unless freeText is false, an Other choice asks for the owner's own answer instead of an option. */
export async function sequential(ui: DialogUi, questions: DialogQuestion[], signal?: AbortSignal) {
  const answers: (DialogAnswer | undefined)[] = questions.map(() => undefined);
  for (const [i, q] of questions.entries()) {
    const labels = q.options.map((o) => o.label);
    const other = q.freeText !== false && !labels.includes(OTHER);
    const option = await ui.select(q.question, other ? [...labels, OTHER] : labels, { signal, timeout: TIMEOUT_MS });
    if (option === undefined || signal?.aborted) break;
    if (other && option === OTHER) {
      const own = (await ui.input("Your answer", "", { signal, timeout: TIMEOUT_MS }))?.trim();
      if (!own || signal?.aborted) break;
      answers[i] = { selected: [], text: own };
      continue;
    }
    if (!labels.includes(option)) break;
    let text = "";
    if (q.freeText) {
      const typed = await ui.input("Optional additional text (leave empty to skip)", "", { signal, timeout: TIMEOUT_MS });
      if (typed === undefined || signal?.aborted) break;
      text = typed;
    }
    answers[i] = { selected: [option], text };
  }
  return answers;
}

/** Dialogs are local to the owner's interactive pane, never a worker question flow. */
export function askUserTool(record: (answer: string) => Promise<string>, pending?: Pick<PendingQuestions, "ask" | "withdraw">,
  dialogBusy: () => boolean = () => false): ToolDefinition {
  return {
    name: "ask_user", label: "Ask the owner", exposure: "model-only", executionMode: "sequential",
    description: "Ask the owner 1-4 focused choice questions together in one dialog instead of asking only in chat; "
      + "batch related questions in one call. Synchronous questions support short headers, string or {label, description} options, "
      + "multiSelect and freeText, including partial answers. Set nonBlocking:true for plain-string, single-selection questions "
      + "with optional freeText: it returns a stable pending ID immediately; the owner answers via /questions while you continue "
      + "independent work. key deduplicates retries; supersedes withdraws a pending question; withdraw closes an unneeded ID. "
      + "Cancellation, timeout or no answer is never consent.",
    promptSnippet: "Present owner questions and clarifications as choices",
    promptGuidelines: [...ASK_USER_GUIDELINES],
    // Legacy provider adapters publish root properties; a root union becomes an empty tool.
    parameters: Type.Object({
      questions: Type.Optional(Type.Array(question, { minItems: 1, maxItems: 4 })),
      nonBlocking: Type.Optional(Type.Boolean()),
      key: Type.Optional(Type.String({ minLength: 1, maxLength: 128 })),
      supersedes: Type.Optional(Type.String()), withdraw: Type.Optional(Type.String({ minLength: 1 })),
    }, { additionalProperties: false, anyOf: [
      Type.Object({ questions: Type.Array(question, { minItems: 1, maxItems: 4 }),
        nonBlocking: Type.Optional(Type.Literal(false)), key: Type.Optional(Type.String({ minLength: 1, maxLength: 128 })),
        supersedes: Type.Optional(Type.String()) }, { additionalProperties: false }),
      Type.Object({ questions: Type.Array(deferredQuestion, { minItems: 1, maxItems: 4 }),
        nonBlocking: Type.Literal(true), key: Type.Optional(Type.String({ minLength: 1, maxLength: 128 })),
        supersedes: Type.Optional(Type.String()) }, { additionalProperties: false }),
      Type.Object({ withdraw: Type.String({ minLength: 1 }) }, { additionalProperties: false }),
    ] }),
    async execute(_id, params: { questions: Question[]; nonBlocking?: false; key?: string; supersedes?: string } | { questions: ChoiceQuestion[]; nonBlocking: true; key?: string; supersedes?: string } | { withdraw: string }, signal, _update, ctx) {
      const noAnswer = () => ({ content: [{ type: "text" as const, text: "no answer" }], details: { status: "no answer" } });
      if ("withdraw" in params) {
        if (!pending) throw new Error("pending_questions_unavailable");
        const status = pending.withdraw(params.withdraw);
        return { content: [{ type: "text", text: `${params.withdraw}: ${status}; no owner answer` }], details: { id: params.withdraw, status } };
      }
      if (!ctx.hasUI || signal?.aborted) return noAnswer();
      if (params.nonBlocking) {
        if (ctx.mode !== "tui" || !pending) return { ...noAnswer(), details: { status: "no answer", reason: "pending UI unavailable" } };
        for (const q of params.questions) {
          if ("multiSelect" in q || "header" in q || q.options.some(option => typeof option !== "string")) {
            throw new Error("deferred_questions_require_plain_single_selection; use the synchronous dialog for rich questions");
          }
          if (new Set(q.options).size !== q.options.length) throw new Error("Options must be distinct");
        }
        const q = pending.ask(_id, params.questions, params.key, params.supersedes);
        return { content: [{ type: "text", text: `${q.id}: ${q.status}. Owner UI: /questions. Continue only independent work; no answer authorizes anything.` }],
          details: { id: q.id, status: q.status, expires: q.expires } };
      }
      if (dialogBusy()) return { ...noAnswer(), details: { status: "no answer", reason: "another owner/permission dialog is open" } };
      const questions = params.questions.map(normalize);
      const shown = (await askDialog(ctx.ui, questions, signal))?.answers;
      if (signal?.aborted && shown === undefined) return noAnswer();
      const raw = shown ?? await sequential(ctx.ui, questions, signal);
      const answers: Answer[] = [];
      const unanswered: string[] = [];
      raw.forEach((a, i) => {
        const q = questions[i]!;
        if (!a || (a.selected.length === 0 && !a.text.trim())) { unanswered.push(q.question); return; }
        // The owner's own typed answer carries no option: nothing is invented in its place.
        if (a.selected.length === 0) { answers.push({ question: q.question, text: a.text }); return; }
        answers.push({
          question: q.question, option: a.selected.join(", "),
          ...(q.multiSelect ? { options: a.selected } : {}), ...(a.text ? { text: a.text } : {}),
        });
      });
      if (answers.length === 0) return noAnswer();
      const lines = answers.map((a) => a.option === undefined ? `${a.question}: ${a.text ?? ""}`
        : `${a.question}: ${a.option}${a.text ? `\n${a.text}` : ""}`);
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
