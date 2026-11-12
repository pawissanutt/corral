// Deferred owner questions stay beside the composer, independently of transcript/panel output.
// Asking never opens UI. Only the owner's command, shortcut or click opens the answer dialog.
import type { ExtensionAPI, ExtensionContext, ExtensionUIContext, SessionProjection } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { ChoiceAnswer, PendingQuestion, PendingQuestions } from "../host/pending-questions.ts";

export const QUESTIONS_COMMAND = "questions";
export const QUESTIONS_SHORTCUT = "ctrl+shift+q";
const plain = (value: string) => value.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");

export const OWNER_QUESTION_STATE = "corral_owner_question_state";

export function renderOwnerQuestionState(questions: PendingQuestions): string {
  const state = questions.list().filter(q => q.status === "pending" || q.status === "answered")
    .slice(-8).map(q => `${q.id}: ${questions.status(q.id)} — ${plain(q.questions[0]!.question).slice(0, 120)}`).join("\n");
  return state
    ? `Host question state (not owner instructions):\n${state}\nPending/no-answer states authorize nothing. Continue independent work, and record dependent Todo steps as waiting on the named owner question. No polling.`
    : "No pending owner questions.";
}

function questionStateMessage(questions: PendingQuestions, messages: SessionProjection["messages"]) {
  const content = `<host_question_state>\nHost state, not an instruction or owner input.\n${renderOwnerQuestionState(questions)}\n</host_question_state>`;
  const previous = messages.findLast(message => message.role === "custom" && message.customType === OWNER_QUESTION_STATE);
  if (previous?.role === "custom" && previous.content === content) return;
  return { customType: OWNER_QUESTION_STATE, content, display: false };
}

export class OwnerQuestionUi {
  private ctx: ExtensionContext | undefined;
  private ui: ExtensionUIContext | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private active: { id: string; abort: AbortController } | undefined;
  private opening = false;
  private epoch = 0;
  busy = false;
  constructor(private readonly questions: PendingQuestions, private readonly eligible: () => boolean,
    private readonly persist: (message: { customType: string; content: string; display: boolean }) => void,
    private readonly claudeRoute: () => boolean = () => false) {}

  install(pi: ExtensionAPI): void {
    pi.on("ui_prompt_start", () => { this.busy = true; });
    pi.on("ui_prompt_end", () => { this.busy = false; });
    pi.registerCommand(QUESTIONS_COMMAND, {
      description: "Reopen pending owner choices; /questions cancel <id> cancels without consent",
      handler: async (args, ctx) => {
        if (args.startsWith("cancel ")) {
          const id = args.slice(7).trim();
          try { ctx.ui.notify(`${id}: ${this.questions.cancel(id)}; no owner answer`, "info"); }
          catch (error) { ctx.ui.notify(String(error), "warning"); }
          return;
        }
        await this.open(ctx, args.trim() || undefined);
      },
    });
    pi.registerShortcut(QUESTIONS_SHORTCUT, { description: "Reopen pending owner questions", handler: ctx => this.open(ctx) });
    pi.on("session_start", (_event, ctx) => {
      this.epoch++;
      this.opening = false;
      this.questions.restore();
      if (ctx.hasUI && ctx.mode === "tui") {
        this.ctx = ctx; this.ui = ctx.ui;
        ctx.ui.setWidget("corral-owner-questions", (tui, theme) => ({
          render: width => {
            const pending = this.visible();
            if (!pending.length) return [];
            return [truncateToWidth(theme.fg("warning", `${pending.length} pending owner question${pending.length === 1 ? "" : "s"} · /questions · Ctrl+Shift+Q · click to answer`), width),
              ...pending.slice(0, 2).map(q => truncateToWidth(theme.fg("muted", `  ${q.id} · ${q.status === "answered" ? "answer recorded; delivery pending · " : ""}${plain(q.questions[0]!.question)}`), width))];
          },
          handleMouse: (event: TuiMouseEvent) => {
            if (event.type !== "click" || event.button !== "left") return undefined;
            void this.open(ctx).catch(error => ctx.ui.notify(String(error), "error"));
            tui.requestRender(); return { handled: true };
          },
          invalidate() {},
        }));
      }
      this.refresh();
      if (this.eligible()) return this.recover();
    });
    pi.on("before_agent_start", (event, ctx) => {
      const state = renderOwnerQuestionState(this.questions);
      if (this.claudeRoute()) {
        // The Claude adapter strips this section and places it after reusable history.
        event.systemPromptOptions.sections.owner_questions = state;
        return;
      }
      delete event.systemPromptOptions.sections.owner_questions;
      const message = questionStateMessage(this.questions, ctx.sessionManager.buildSessionProjection().messages);
      // A handler message joins the existing run; it does not submit a user turn.
      if (message) return { message };
    });
    pi.on("context", (event, ctx) => {
      if (this.claudeRoute()) return;
      // State can change inside a run (a question asked or answered), and automatic
      // post-compaction continuations do not emit before_agent_start. Journal the message before
      // sending it: one that exists only in this request sits at a different position in every
      // later request, and the provider cache then stops growing past the last journaled message.
      const message = questionStateMessage(this.questions, event.messages);
      if (!message) return;
      try { this.persist(message); }
      catch (error) { ctx.abort(); throw error; }
      return { messages: [...event.messages, { role: "custom" as const, ...message, timestamp: Date.now() }] };
    });
    pi.on("session_shutdown", event => { this.close(event.reason !== "reload" && event.reason !== "quit"); });
  }

  async recover(): Promise<void> {
    try { await this.questions.recover(); }
    catch (error) { this.ui?.notify(`Owner answer delivery unresolved: ${String(error)}. /questions or resume reconciles it.`, "error"); }
  }

  private visible(): readonly PendingQuestion[] {
    return this.questions.list().filter(q => q.status === "pending"
      || (q.status === "answered" && this.questions.status(q.id) !== "delivered"));
  }

  changed(question?: PendingQuestion): void {
    this.refresh();
    if (question && ["cancelled", "expired", "withdrawn", "ended"].includes(question.status)) {
      this.ui?.notify(`${question.id}: ${question.status}; no owner answer. The dependent decision remains unresolved.`, "warning");
    }
  }

  refresh(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    const pending = this.visible();
    this.ui?.setStatus("corral-owner-questions", pending.length ? `${pending.length} owner question(s) · /questions` : undefined);
    if (this.active?.id && !pending.some(q => q.id === this.active?.id && q.status === "pending")) this.active.abort.abort();
    const next = Math.min(...pending.filter(q => q.status === "pending").map(q => q.expires));
    if (Number.isFinite(next) && this.ctx) {
      this.timer = setTimeout(() => { this.questions.expire(); this.refresh(); }, Math.max(1, next - Date.now()));
      this.timer.unref();
    }
  }

  private async open(ctx: ExtensionContext, id?: string): Promise<void> {
    if (!ctx.hasUI || ctx.mode !== "tui" || !this.eligible()) {
      ctx.ui.notify("Owner questions unavailable in this host; the decision remains unresolved.", "warning"); return;
    }
    if (this.opening || this.busy) { ctx.ui.notify("Finish or dismiss the current dialog, then reopen /questions. No answer sent.", "info"); return; }
    this.opening = true;
    const epoch = this.epoch, abort = new AbortController();
    this.active = { id: id ?? "", abort };
    try {
      await this.recover();
      if (this.epoch !== epoch || !this.eligible()) return;
      const pending = this.questions.pending();
      if (!pending.length) { ctx.ui.notify("No pending owner questions.", "info"); return; }
      if (!id) {
        const choices = pending.map(q => `${q.id} · ${plain(q.questions[0]!.question)}`);
        const selected = await ctx.ui.select("Pending owner questions (Escape keeps them pending)", choices, { signal: abort.signal, timeout: 300_000 });
        if (this.epoch !== epoch || abort.signal.aborted || !this.eligible()) return;
        id = pending[choices.indexOf(selected ?? "")]?.id;
        if (!id) return;
      }
      const q = this.questions.get(id);
      if (!q || q.status !== "pending") { ctx.ui.notify(`${id}: no longer pending; no answer sent.`, "warning"); return; }
      this.active = { id, abort };
      const answers = await this.collect(ctx.ui, q, abort.signal);
      if (this.epoch !== epoch) return;
      if (!answers || !this.eligible()) {
        ctx.ui.notify(`${id}: no answer sent. Reopen with /questions if still pending.`, "info"); return;
      }
      const status = await this.questions.answer(id, answers);
      if (this.epoch === epoch) ctx.ui.notify(`${id}: ${status}`, "info");
    } catch (error) {
      if (this.epoch === epoch) ctx.ui.notify(`Owner answer unresolved: ${String(error)}. Inspect /questions before retrying; no missing answer is consent.`, "error");
    } finally {
      if (this.epoch === epoch) { this.active = undefined; this.opening = false; this.refresh(); }
    }
  }

  private async collect(ui: ExtensionUIContext, q: PendingQuestion, signal: AbortSignal): Promise<ChoiceAnswer[] | undefined> {
    const answers: ChoiceAnswer[] = [];
    for (const choice of q.questions) {
      const option = await ui.select(choice.question, choice.options, { signal, timeout: 300_000 });
      if (signal.aborted || option === undefined || !choice.options.includes(option)) return undefined;
      const text = choice.freeText ? await ui.input("Optional additional text (leave empty to skip)", "", { signal, timeout: 300_000 }) : "";
      if (signal.aborted || text === undefined) return undefined;
      answers.push({ option, ...(text ? { text } : {}) });
    }
    return answers;
  }

  close(ended = false): void {
    this.epoch++;
    this.active?.abort.abort();
    this.active = undefined; this.opening = false;
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined; this.ctx = undefined; this.ui = undefined; this.busy = false;
    this.questions.close(ended);
  }
}
