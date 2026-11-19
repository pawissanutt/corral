import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import {
  CARD_OPTION_MAX, cardOptions, cardQuestion, invalidAnswer, optionId, singleAnswer, type AnswerStatus, type CardAnswer, type CardEntry,
} from "./cards.ts";

type DialogOptions = NonNullable<Parameters<ExtensionUIContext["select"]>[2]>;
export type CardDraft = Omit<CardEntry["card"], "id" | "opened_at" | "expires_at">;
export type AnswerSource = { source: "terminal" } | { source: "remote"; device?: string };
export type DialogResolution =
  | { kind: "answered"; answers: readonly CardAnswer[]; by: AnswerSource }
  | { kind: "cancelled"; reason: "dismissed" | "expired" | "closed"; by?: AnswerSource };

export interface OpenDialog {
  readonly id: string;
  readonly result: Promise<DialogResolution>;
  readonly settled: boolean;
  answer(answers: readonly CardAnswer[]): AnswerStatus;
  cancel(): AnswerStatus;
}
export type Prompt =
  | { kind: "select"; title: string; options: readonly string[]; body?: string }
  | { kind: "confirm"; title: string; body?: string }
  | { kind: "input"; title: string; placeholder?: string };
export interface PromptDialog extends Omit<OpenDialog, "answer"> {
  readonly value: Promise<string | undefined>;
  answer(value: string): AnswerStatus;
}
export interface ExistingDialog {
  readonly id: string;
  answer(answers: readonly CardAnswer[]): Promise<AnswerStatus>;
  dismiss(): Promise<AnswerStatus>;
  /** Detach this view without deciding for the owner. */
  dispose(): void;
}
interface ExistingCards {
  entries(): CardEntry[];
  absent(id: string): AnswerStatus;
  shared(): boolean;
}
export interface OpenOptions extends DialogOptions {
  validate?(answers: readonly CardAnswer[], by: AnswerSource): void;
  /** The pre-S13 headless socket keeps its existing status on a repeated answer. */
  shared?: boolean;
}
type Listener = (result: DialogResolution) => void;
const terminal = { source: "terminal" } as const;
const TIMEOUT_MAX = 2 ** 31 - 1;
const RECENT_LIMIT = 128;

export class RemoteDialogs {
  private readonly dialogs = new Map<string, { entry: CardEntry; close(): void }>();
  private readonly listeners = new Map<string, Set<Listener>>();
  private readonly answering = new Set<string>();
  private readonly recent = new Map<string, "answered" | "expired">();
  private closed = false;

  constructor(private readonly changed: () => void, private readonly existing?: ExistingCards) {}

  entries(): CardEntry[] {
    if (this.closed) return [];
    return [...(this.existing?.entries() ?? []), ...Array.from(this.dialogs.values(), dialog => dialog.entry)];
  }

  watch(matches: (card: Readonly<CardEntry["card"]>) => boolean, listener: Listener): ExistingDialog | undefined {
    if (this.closed) return;
    const id = this.existing?.entries().find(({ card }) => matches(card))?.card.id;
    if (!id) return;
    const listeners = this.listeners.get(id) ?? new Set<Listener>();
    listeners.add(listener);
    this.listeners.set(id, listeners);
    return {
      id,
      answer: answers => this.resolve(id, answers, false, terminal),
      dismiss: () => this.resolve(id, [], true, terminal),
      dispose: () => {
        listeners.delete(listener);
        if (!listeners.size && this.listeners.get(id) === listeners) this.listeners.delete(id);
      },
    };
  }

  async resolve(id: string, answers: readonly CardAnswer[], dismiss: boolean, by: AnswerSource,
    beforeQuestion: () => Promise<unknown> = async () => {}): Promise<AnswerStatus> {
    if (this.closed) return "missing";
    const dialog = this.dialogs.get(id);
    if (dialog) return dialog.entry.answer(answers, dismiss, beforeQuestion, by);
    const recent = this.recent.get(id);
    if (recent === "expired") return recent;
    if (this.answering.has(id)) invalidAnswer("the card is already being answered");
    const entry = this.existing?.entries().find(({ card }) => card.id === id);
    // A source may reopen the same ID after a failed attempt. Only the source may reopen it.
    if (!entry) {
      if (recent === "answered") invalidAnswer("the card has already been answered");
      return this.existing?.absent(id) ?? "missing";
    }
    const shared = this.existing?.shared() || this.listeners.has(id);
    if (shared) this.answering.add(id);
    try {
      const status = await entry.answer(answers, dismiss, beforeQuestion, by);
      if (shared && !this.closed) {
        if (status === "delivered" || status === "answered" || status === "started") {
          this.remember(id, "answered");
          this.publish(id, dismiss ? { kind: "cancelled", reason: "dismissed", by } : { kind: "answered", answers, by });
        } else if (status === "dismissed") {
          this.remember(id, "answered");
          this.publish(id, { kind: "cancelled", reason: "dismissed", by });
        }
      }
      return status;
    } finally {
      if (shared) this.answering.delete(id);
    }
  }

  open(card: CardDraft, opts: OpenOptions = {}): OpenDialog {
    const { signal, timeout, shared = true, validate } = opts;
    const id = `dialog-${crypto.randomUUID()}`, now = Date.now();
    const expires = timeout !== undefined && Number.isFinite(timeout) && timeout > 0 ? Math.min(now + timeout, 8.64e15) : undefined;
    const { promise: result, resolve } = Promise.withResolvers<DialogResolution>();
    let resolution: DialogResolution | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = (value: DialogResolution) => {
      if (resolution) return;
      resolution = value;
      const removed = this.dialogs.delete(id);
      clearTimeout(timer);
      signal?.removeEventListener("abort", expire);
      if (removed && value.kind === "answered" && shared) this.remember(id, "answered");
      if (removed && value.kind === "cancelled" && value.reason === "expired") this.remember(id, "expired");
      resolve(value);
      if (removed) this.changed();
    };
    const expire = () => finish({ kind: "cancelled", reason: "expired" });
    const close = () => finish({ kind: "cancelled", reason: "closed" });
    const submit = (answers: readonly CardAnswer[], dismiss: boolean, by: AnswerSource): AnswerStatus => {
      if (!dismiss && resolution?.kind === "answered" && shared) invalidAnswer("the card has already been answered");
      if (resolution) return resolution.kind === "cancelled" && resolution.reason === "expired" ? "expired" : "missing";
      if (signal?.aborted || expires !== undefined && expires <= Date.now()) { expire(); return "expired"; }
      if (!dismiss) validate?.(answers, by);
      finish(dismiss ? { kind: "cancelled", reason: "dismissed", by } : { kind: "answered", answers, by });
      return dismiss ? "dismissed" : "answered";
    };
    const schedule = () => {
      if (expires === undefined) return;
      const wait = expires - Date.now();
      if (wait <= 0) expire();
      else {
        timer = setTimeout(schedule, Math.min(wait, TIMEOUT_MAX));
        timer.unref();
      }
    };
    if (this.closed) close();
    else if (signal?.aborted) expire();
    else {
      this.dialogs.set(id, { close, entry: {
        card: { ...card, id, opened_at: new Date(now).toISOString(), expires_at: expires === undefined ? undefined : new Date(expires).toISOString() },
        answer: (answers, dismiss, _before, by = { source: "remote" }) => submit(answers, dismiss, by),
      } });
      signal?.addEventListener("abort", expire, { once: true });
      schedule();
      this.changed();
    }
    return { id, result, get settled() { return resolution !== undefined; },
      answer: answers => submit(answers, false, terminal), cancel: () => submit([], true, terminal) };
  }

  openPrompt(prompt: Prompt, opts?: DialogOptions): PromptDialog {
    return this.prompt(prompt, opts, true);
  }

  private prompt(prompt: Prompt, opts: DialogOptions | undefined, shared: boolean): PromptDialog {
    const { kind, title } = prompt;
    const labels = kind === "select" ? [...prompt.options] : [];
    const options = kind === "select" ? cardOptions(labels.slice(0, CARD_OPTION_MAX).map(label => ({ label })))
      : kind === "confirm" ? [{ id: "yes", label: "Yes", destructive: false }, { id: "no", label: "No", destructive: false }] : [];
    let body = kind === "input" ? prompt.placeholder : prompt.body;
    if (labels.length > CARD_OPTION_MAX) {
      const limit = `Showing the first ${CARD_OPTION_MAX} of ${labels.length} choices.`;
      body = body ? `${body}\n\n${limit}` : limit;
    }
    const decode = (answers: readonly CardAnswer[], by: AnswerSource): string => {
      const { options, text } = singleAnswer(answers);
      if (kind === "input") {
        if (options.length || text === undefined) invalidAnswer("enter text");
        return text;
      }
      const id = options[0];
      if (id === undefined || text !== undefined) invalidAnswer("select one option");
      if (kind === "confirm") {
        if (id === "yes") return "Yes";
        if (id === "no") return "No";
        invalidAnswer("choose yes or no");
      }
      const selected = labels.find((label, i) => (by.source === "terminal" || i < CARD_OPTION_MAX) && optionId(label, i) === id);
      if (selected === undefined) invalidAnswer("select a listed option");
      return selected;
    };
    const dialog = this.open({ kind, title, body, questions: [cardQuestion(title, options, kind === "input")] },
      { ...opts, shared, validate: decode });
    return { id: dialog.id, result: dialog.result, cancel: dialog.cancel, get settled() { return dialog.settled; },
      value: dialog.result.then(result => result.kind === "answered" ? decode(result.answers, result.by) : undefined),
      answer: text => {
        if (kind === "input") return dialog.answer([{ options: [], text }]);
        const index = labels.indexOf(text);
        return dialog.answer([{ options: [kind === "confirm" ? text === "Yes" ? "yes" : text === "No" ? "no" : "" :
          index < 0 ? "" : optionId(text, index)] }]);
      },
    };
  }

  context(base: ExtensionUIContext, notify: ExtensionUIContext["notify"]): ExtensionUIContext {
    return { ...base, notify,
      select: (title, options, opts) => this.prompt({ kind: "select", title, options }, opts, false).value,
      confirm: async (title, body, opts) => await this.prompt({ kind: "confirm", title, body }, opts, false).value === "Yes",
      input: (title, placeholder, opts) => this.prompt({ kind: "input", title, placeholder }, opts, false).value,
    };
  }

  close(): void {
    this.closed = true;
    for (const dialog of this.dialogs.values()) dialog.close();
    for (const id of this.listeners.keys()) this.publish(id, { kind: "cancelled", reason: "closed" });
  }

  private publish(id: string, result: DialogResolution): void {
    const listeners = this.listeners.get(id);
    this.listeners.delete(id);
    for (const listener of listeners ?? []) {
      try { listener(result); } catch { /* A presentation observer cannot change a committed answer. */ }
    }
  }

  private remember(id: string, status: "answered" | "expired"): void {
    this.recent.set(id, status);
    if (this.recent.size > RECENT_LIMIT) this.recent.delete(this.recent.keys().next().value!);
  }
}
