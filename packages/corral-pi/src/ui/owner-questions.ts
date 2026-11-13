// Deferred owner questions stay beside the composer, independently of transcript/panel output, in the same
// docked dialog form as every other question the owner is asked. Asking never opens UI. Only the owner's
// command, key or click opens the answer dialog. With native panels the card is a dock item the panels
// focus like the docked build choice; plain hosts keep it as a widget of its own.
import type { ExtensionAPI, ExtensionContext, ExtensionUIContext, SessionProjection } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, matchesKey, type Component, type KeyId, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { askDialog, sequential } from "../host/ask-user.ts";
import type { ChoiceAnswer, PendingQuestion, PendingQuestions } from "../host/pending-questions.ts";
import { choiceLines, PASTE_START } from "./build/choice-dock.ts";
import { freeKey, keyLabel } from "./keymap.ts";

export const QUESTIONS_COMMAND = "questions";
/**
 * The owner-question key ("answer"), unshifted so every terminal delivers it as itself. Not Alt+Q: Pi's
 * Windows and WSL table binds Alt+Q to pulling back queued input.
 */
export const QUESTIONS_KEY: KeyId = "alt+a";
/** Alt+A unless the owner bound it to an editor or app action. */
export function questionsKey(): KeyId | undefined {
  return freeKey(QUESTIONS_KEY);
}
const plain = (value: string) => value.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
/** Questions listed in the dock; the header counts the rest. */
const LISTED = 3;

/**
 * The waiting questions as a docked dialog: the count in the header, how to answer as its note (shortening
 * to `/questions`, which shows at every width), and one option per question, the question on its label row
 * and under it the full id the owner answers by (`/questions <id>`). `selected` marks a question while the
 * card has the keys; `short` gives each question one row. `hits` maps a line to the question drawn on it.
 */
export function ownerQuestionLines(pending: readonly PendingQuestion[], width: number, selected = -1,
  maxRows = 3 + LISTED * 4, short = false): { readonly lines: string[]; readonly hits: ReadonlyMap<number, string> } {
  if (!pending.length) return { lines: [], hits: new Map() };
  const listed = pending.slice(0, LISTED), count = pending.length;
  const key = questionsKey(), cap = key ? keyLabel(key) : undefined;
  const frame = choiceLines({
    key: "owner-questions",
    title: count === 1 ? "An owner question is waiting" : `${count} owner questions are waiting`,
    short: count === 1 ? "Question waiting" : `${count} questions waiting`,
    metaFit: cap ? [`click, /questions or ${cap} to answer`, `/questions or ${cap}`, "/questions"]
      : ["click or /questions to answer", "/questions"],
    stacked: true,
    options: listed.map(q => ({ label: plain(q.questions[0]!.question),
      detail: q.status === "answered" ? `${q.id} · answer recorded; delivery pending` : `/questions ${q.id}` })),
  }, selected, width, maxRows, short);
  const hits = new Map<number, string>();
  for (const [line, option] of frame.hits) hits.set(line, listed[option]!.id);
  return { lines: frame.lines, hits };
}

/** What the panels' dock reads from the owner questions. */
export interface QuestionSource {
  /** Pending questions and answers still being delivered, oldest first. */
  waiting(): readonly PendingQuestion[];
  /** Opens that pending question's answer dialog, or the list of pending questions without an id. */
  open(id?: string): void;
  /** `listener` runs whenever the waiting questions change; returns its removal. */
  subscribe(listener: () => void): () => void;
}

export interface QuestionDockHost {
  source(): QuestionSource | undefined;
  /** A click focused the card: the other dock items and views give up the keys. */
  focus(): void;
  /** Esc, Tab, typing or an opened question: the composer has the keys again. */
  focusComposer(): void;
  changed(): void;
}

/**
 * The owner-question card as a dock item, under the docked choice's rules: Alt+Down or a click gives it the
 * keys, ↑↓ select, Enter or a digit opens that question, Esc returns to the composer. Function keys, view keys
 * and Alt chords pass through; a printable key is typing and goes to the composer.
 */
export class QuestionDock implements Component {
  focused = false;
  selected = 0;
  private maxRows = 3 + LISTED * 4;
  private short = false;
  private hits: ReadonlyMap<number, string> = new Map();

  constructor(private readonly host: QuestionDockHost) {}

  /** Questions waiting, listed or not. */
  get waiting(): number { return this.host.source()?.waiting().length ?? 0; }

  setLimit(maxRows: number, short: boolean): void { this.maxRows = maxRows; this.short = short; }

  handleKey(data: string): boolean {
    if (!this.focused) return false;
    const listed = this.listed();
    if (!listed.length) { this.focused = false; return false; }
    // Pasted text is typing: the composer takes it, so a later Enter sends the draft instead of opening a question.
    if (data.startsWith(PASTE_START)) { this.leave(); return false; }
    const ch = decodeKittyPrintable(data) ?? data;
    if (matchesKey(data, "escape") || matchesKey(data, "tab")) { this.leave(); return true; }
    if (matchesKey(data, "up") || matchesKey(data, "down")) {
      this.selected = (this.selected + (matchesKey(data, "up") ? -1 : 1) + listed.length) % listed.length;
      this.host.changed(); return true;
    }
    if (matchesKey(data, "enter")) { this.openAt(listed, this.selected); return true; }
    if (/^[0-9]$/.test(ch)) {
      const index = Number(ch) - 1;
      if (index >= 0 && index < listed.length) this.openAt(listed, index);
      return true;
    }
    if (ch.startsWith("\x1b") || /^[\u0000-\u001f\u007f]$/.test(ch)) return false;
    this.leave();
    return false;
  }

  render(width: number): string[] {
    const waiting = this.host.source()?.waiting() ?? [];
    if (!waiting.length) { this.hits = new Map(); this.focused = false; return []; }
    this.selected = Math.min(this.selected, Math.min(LISTED, waiting.length) - 1);
    const frame = ownerQuestionLines(waiting, width, this.focused ? this.selected : -1, this.maxRows, this.short);
    this.hits = frame.hits;
    return frame.lines;
  }

  /** A click gives the card the keys and selects a question; a click on the selected question opens it. */
  handleMouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    if (event.type !== "click" || event.button !== "left") return { handled: true };
    const listed = this.listed(), id = this.hits.get(event.y), index = listed.findIndex(q => q.id === id);
    if (index >= 0 && this.focused && index === this.selected) { this.openAt(listed, index); return { handled: true }; }
    if (index >= 0) this.selected = index;
    this.focused = true;
    this.host.focus();
    this.host.changed();
    return { handled: true };
  }

  invalidate(): void {}

  private listed(): readonly PendingQuestion[] { return (this.host.source()?.waiting() ?? []).slice(0, LISTED); }

  private leave(): void {
    this.focused = false;
    this.host.focusComposer();
    this.host.changed();
  }

  /** A pending question opens its answer dialog; one whose answer is being delivered opens the list. */
  private openAt(listed: readonly PendingQuestion[], index: number): void {
    const question = listed[index];
    if (!question) return;
    this.selected = index;
    this.leave();
    this.host.source()?.open(question.status === "pending" ? question.id : undefined);
  }
}

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
  // Typed drafts survive Escape and reopening for this session; an answered question drops its own.
  private readonly drafts = new Map<string, string[]>();
  busy = false;
  /** True once the panels took the card into their dock: no widget of its own. */
  private docked = false;
  private readonly listeners = new Set<() => void>();
  constructor(private readonly questions: PendingQuestions, private readonly eligible: () => boolean,
    private readonly persist: (message: { customType: string; content: string; display: boolean }) => void,
    private readonly claudeRoute: () => boolean = () => false) {}

  /** Hands the card to the native panels' dock; call before `session_start`. */
  dock(): QuestionSource {
    this.docked = true;
    return {
      waiting: () => this.visible(),
      open: id => {
        const ctx = this.ctx;
        if (ctx) void this.open(ctx, id).catch(error => ctx.ui.notify(String(error), "error"));
      },
      subscribe: listener => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; },
    };
  }

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
    const key = questionsKey();
    if (key) pi.registerShortcut(key, { description: "Reopen pending owner questions", handler: ctx => this.open(ctx) });
    pi.on("session_start", (_event, ctx) => {
      this.epoch++;
      this.opening = false;
      this.questions.restore();
      if (ctx.hasUI && ctx.mode === "tui") {
        this.ctx = ctx; this.ui = ctx.ui;
      }
      if (ctx.hasUI && ctx.mode === "tui" && !this.docked) {
        let hits: ReadonlyMap<number, string> = new Map();
        ctx.ui.setWidget("corral-owner-questions", tui => ({
          render: width => {
            const frame = ownerQuestionLines(this.visible(), width);
            hits = frame.hits;
            return frame.lines;
          },
          handleMouse: (event: TuiMouseEvent) => {
            if (event.type !== "click" || event.button !== "left") return undefined;
            // A pending question's row opens that question; anywhere else opens the list.
            const id = hits.get(event.y), question = id === undefined ? undefined : this.questions.get(id);
            void this.open(ctx, question?.status === "pending" ? id : undefined).catch(error => ctx.ui.notify(String(error), "error"));
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
    for (const listener of this.listeners) listener();
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
      this.drafts.delete(id);
      if (this.epoch === epoch) ctx.ui.notify(`${id}: ${status}`, "info");
    } catch (error) {
      if (this.epoch === epoch) ctx.ui.notify(`Owner answer unresolved: ${String(error)}. Inspect /questions before retrying; no missing answer is consent.`, "error");
    } finally {
      if (this.epoch === epoch) { this.active = undefined; this.opening = false; this.refresh(); }
    }
  }

  /** One dialog for the batch; only a deliberate Submit with every question answered is an answer. */
  private async collect(ui: ExtensionUIContext, q: PendingQuestion, signal: AbortSignal): Promise<ChoiceAnswer[] | undefined> {
    const questions = q.questions.map(choice => ({ question: choice.question, options: choice.options.map(label => ({ label })),
      ...(choice.freeText === undefined ? {} : { freeText: choice.freeText }) }));
    const drafts = this.drafts.get(q.id) ?? [];
    this.drafts.set(q.id, drafts);
    const shown = await askDialog(ui, questions, signal, { escape: "Esc keeps it pending", complete: true, drafts });
    if (signal.aborted) return undefined;
    if (shown && !shown.submitted) return undefined;
    const raw = shown?.answers ?? await sequential(ui, questions, signal);
    if (signal.aborted) return undefined;
    const answers: ChoiceAnswer[] = [];
    for (const a of raw) {
      if (!a) return undefined;
      const text = a.text.trim();
      if (a.selected.length === 0) { if (!text) return undefined; answers.push({ text }); }
      else answers.push({ option: a.selected[0]!, ...(text ? { text } : {}) });
    }
    return answers;
  }

  close(ended = false): void {
    this.epoch++;
    this.active?.abort.abort();
    this.active = undefined; this.opening = false; this.drafts.clear();
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined; this.ctx = undefined; this.ui = undefined; this.busy = false;
    this.questions.close(ended);
  }
}
