import type { ExtensionToolContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { ASK_USER_GUIDELINES } from "../roles/common/ask-user.ts";
import { AskDialog, type Choice, type DialogAnswer, type DialogOptions, type DialogQuestion, type DialogResult } from "./ask-user-dialog.ts";
import { ANSWER_MAX_BYTES, Choice as PendingChoice, type ChoiceQuestion, type PendingQuestions } from "./pending-questions.ts";
import { cardOptions, invalidAnswer, optionId, type CardAnswer } from "./remote/cards.ts";
import type { OpenDialog, RemoteDialogs } from "./remote/dialogs.ts";

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

function normalize(q: Question): DialogQuestion {
  const options = q.options.map((o) => (typeof o === "string" ? { label: o } : o));
  if (new Set(options.map((o) => o.label)).size !== options.length) throw new Error("Options must be distinct");
  return { ...q, options };
}

type DialogUi = Pick<ExtensionToolContext["ui"], "custom" | "select" | "input">;

/** Where a dialog shows and how many rows it may take: presentation only, read at each open. The caller's
 * escape, completion and drafts stay authoritative. */
export type DialogPresentationSource = () => Pick<DialogOptions, "surface" | "rows">;

function remoteAnswers(questions: readonly DialogQuestion[], answers: readonly CardAnswer[]): DialogAnswer[] {
  if (answers.length !== questions.length) invalidAnswer("answer each question");
  const decoded = answers.map(({ options, text = "" }, i) => {
    const question = questions[i]!;
    if ((!question.multiSelect && options.length > 1) || new Set(options).size !== options.length ||
        (options.length ? text && question.freeText !== true : question.freeText === false || !text.trim())) {
      invalidAnswer("choose listed options or use free text if allowed");
    }
    const selected = options.map(id => {
      const option = question.options.find(({ label }, index) => optionId(label, index) === id);
      if (!option) invalidAnswer("choose a listed option");
      return option.label;
    });
    return { selected, text };
  });
  const recorded = decoded.map(({ selected, text }, i) =>
    `${questions[i]!.question}: ${selected.length ? selected.join(", ") + (text ? `\n${text}` : "") : text}`).join("\n");
  if (Buffer.byteLength(recorded) > ANSWER_MAX_BYTES) invalidAnswer("the completed answer is too large");
  return decoded;
}

/** One dialog for the whole batch; Escape, abort or timeout keeps what was answered. Undefined when the
 * host shows no custom component (a select-only or no-op UI context, or RPC mode). */
export async function askDialog(ui: DialogUi, questions: DialogQuestion[], signal?: AbortSignal,
  options?: DialogOptions, dialogs?: RemoteDialogs): Promise<DialogResult | undefined> {
  if (typeof ui.custom !== "function") return undefined;
  let current: AskDialog | undefined;
  const shared: { dialog?: OpenDialog; active: boolean } = { active: true };
  let timer: ReturnType<typeof setTimeout> | undefined;
  const stop = () => current?.finish(false);
  signal?.addEventListener("abort", stop, { once: true });
  try {
    const result = await ui.custom<DialogResult | undefined>((tui, _theme, _keys, done) => {
      const arm = () => { clearTimeout(timer); timer = setTimeout(stop, TIMEOUT_MS); };
      if (dialogs) {
        let terminalResult: DialogResult | undefined;
        let wireAnswers: DialogAnswer[] | undefined;
        const opened = dialogs.open({
          kind: "question", title: questions[0]?.question ?? "Owner question", tool: "ask_user",
          questions: questions.map(q => ({ text: q.question, header: q.header, multi: q.multiSelect === true,
            free_text: q.freeText !== false, options: cardOptions(q.options.map(({ label, description }) => ({ label, detail: description }))) })),
        }, { signal, validate: (answers, by) => {
          if (by.source === "remote") wireAnswers = remoteAnswers(questions, answers);
        } });
        shared.dialog = opened;
        current = new AskDialog(questions, result => {
          if (opened.settled) return;
          terminalResult = result;
          if (result.submitted) opened.answer(result.answers.map((a, i) => ({
            options: a?.selected.map(label => optionId(label, questions[i]!.options.findIndex(option => option.label === label))) ?? [],
            ...(a?.text ? { text: a.text } : {}),
          })));
          else opened.cancel();
        }, () => { arm(); tui.requestRender(); }, options);
        void opened.result.then(resolution => {
          if (!shared.active) return;
          current?.finish(false);
          done(resolution.kind === "answered" && resolution.by.source === "remote"
            ? { submitted: true, answers: wireAnswers! }
            : terminalResult ?? { submitted: false, answers: questions.map(() => undefined) });
        });
      } else current = new AskDialog(questions, done, () => { arm(); tui.requestRender(); }, options);
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
    shared.active = false;
    shared.dialog?.cancel();
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
type AskParams = { questions: Question[]; nonBlocking?: false }
  | { questions: ChoiceQuestion[]; nonBlocking: true } | { withdraw: string };
type DeferredAsk = (id: string, params: AskParams, signal?: AbortSignal) => Promise<{
  content: { type: "text"; text: string }[]; details: Record<string, unknown>;
}>;

/** Deferred parameters cannot include features only the pane dialog understands. */
function validateDeferredQuestions(questions: readonly ChoiceQuestion[]): void {
  for (const q of questions) {
    if ("multiSelect" in q || "header" in q || q.options.some(option => typeof option !== "string")) {
      throw new Error("deferred_questions_require_plain_single_selection; use the synchronous dialog for rich questions");
    }
    if (new Set(q.options).size !== q.options.length) throw new Error("Options must be distinct");
  }
}

export function localQuestionAsk(pending: PendingQuestions): DeferredAsk {
  return async (id, params, signal) => {
    signal?.throwIfAborted();
    if ("withdraw" in params) {
      const status = pending.withdraw(params.withdraw);
      return { content: [{ type: "text", text: `${params.withdraw}: ${status}` }], details: { status } };
    }
    if (!params.questions?.length || params.questions.length > 4) throw new Error("ask_user requires 1-4 questions");
    if (params.nonBlocking) validateDeferredQuestions(params.questions);
    const questions = params.nonBlocking ? params.questions.map(q => PendingChoice.parse(q)) : params.questions.map(q => {
      if ("multiSelect" in q && q.multiSelect) {
        throw new Error("ask_user takes one selection per question; ask separate questions instead");
      }
      const normalized = normalize(q);
      return PendingChoice.parse({ ...q, options: normalized.options.map(option => option.label) });
    });
    const question = pending.ask(questions.length === 1 ? `${id}:0` : id, questions, "ask_user");
    return { content: [{ type: "text", text: `Waiting for the owner: ${question.id}. Continue only independent work.` }],
      details: { status: "pending", questions: [question.id], id: question.id } };
  };
}

/** Interactive panes keep synchronous dialogs; deferred and headless asks use the pending store. */
export function askUserTool(record: (answer: string) => Promise<string>, pending?: Pick<PendingQuestions, "ask" | "withdraw">,
  dialogBusy: () => boolean = () => false, presentation?: DialogPresentationSource, deferred?: DeferredAsk,
  headless: () => boolean = () => false, dialogs?: () => RemoteDialogs | undefined): ToolDefinition {
  return {
    name: "ask_user", label: "Ask the owner", exposure: "model-only", executionMode: "sequential",
    description: "Ask the owner 1-4 focused choice questions instead of asking only in chat. "
      + "In a pane, synchronous questions use a dialog and support short headers, string or {label, description} options, "
      + "multiSelect and freeText, including partial answers. Set nonBlocking:true for plain-string, single-selection questions "
      + "with optional freeText. Deferred (or headless) questions return pending IDs for the owner to answer in the app or via /questions. "
      + "Continue only independent work. Withdraw closes an unneeded ID. Cancellation, timeout or no answer is never consent.",
    promptSnippet: "Present owner questions and clarifications as choices",
    promptGuidelines: [...ASK_USER_GUIDELINES],
    // Legacy provider adapters publish root properties; a root union becomes an empty tool.
    parameters: Type.Object({
      questions: Type.Optional(Type.Array(question, { minItems: 1, maxItems: 4 })),
      nonBlocking: Type.Optional(Type.Boolean()),
      withdraw: Type.Optional(Type.String({ minLength: 1 })),
    }, { additionalProperties: false, anyOf: [
      Type.Object({ questions: Type.Array(question, { minItems: 1, maxItems: 4 }),
        nonBlocking: Type.Optional(Type.Literal(false)) }, { additionalProperties: false }),
      Type.Object({ questions: Type.Array(deferredQuestion, { minItems: 1, maxItems: 4 }),
        nonBlocking: Type.Literal(true) }, { additionalProperties: false }),
      Type.Object({ withdraw: Type.String({ minLength: 1 }) }, { additionalProperties: false }),
    ] }),
    async execute(id, params: AskParams, signal, _update, ctx) {
      const noAnswer = () => ({ content: [{ type: "text" as const, text: "no answer" }], details: { status: "no answer" } });
      if ("withdraw" in params) {
        if (!pending) throw new Error("pending_questions_unavailable");
        const status = pending.withdraw(params.withdraw);
        return { content: [{ type: "text", text: `${params.withdraw}: ${status}; no owner answer` }], details: { id: params.withdraw, status } };
      }
      if (signal?.aborted) return noAnswer();
      if (deferred && (headless() || !ctx.hasUI || params.nonBlocking)) return deferred(id, params, signal);
      if (headless() || !ctx.hasUI) return noAnswer();
      if (params.nonBlocking) {
        if (ctx.mode !== "tui" || !pending) return { ...noAnswer(), details: { status: "no answer", reason: "pending UI unavailable" } };
        validateDeferredQuestions(params.questions);
        const q = pending.ask(id, params.questions, "ask_user");
        return { content: [{ type: "text", text: `${q.id}: ${q.status}. Owner UI: /questions. Continue only independent work; no answer authorizes anything.` }],
          details: { id: q.id, status: q.status, expires: q.expires } };
      }
      if (dialogBusy()) return { ...noAnswer(), details: { status: "no answer", reason: "another owner dialog is open" } };
      const questions = params.questions.map(normalize);
      const shown = (await askDialog(ctx.ui, questions, signal, presentation?.(), dialogs?.()))?.answers;
      const raw = shown ?? (signal?.aborted ? [] : await sequential(ctx.ui, questions, signal));
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
      if (Buffer.byteLength(answer) > ANSWER_MAX_BYTES) return noAnswer();
      const recorded = await record(answer);
      const status = unanswered.length ? "partial" : "answered";
      return { content: [{ type: "text", text: recorded }], details: { status, answers, ...(unanswered.length ? { unanswered } : {}) } };
    },
  };
}
