import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import {
  CARD_OPTION_MAX, cardOptions, cardQuestion, invalidAnswer, optionId, singleAnswer, type CardAnswer, type CardEntry, type CardOption,
} from "./cards.ts";

type DialogOptions = Parameters<ExtensionUIContext["select"]>[2];
const TIMEOUT_MAX = 2 ** 31 - 1;
const RECENT_EXPIRED = 64;
const confirmOptions: CardOption[] = [
  { id: "yes", label: "Yes", destructive: false }, { id: "no", label: "No", destructive: false },
];

export class RemoteDialogs {
  private readonly dialogs = new Map<string, { entry: CardEntry; close(): void }>();
  private readonly expired = new Set<string>();
  private closed = false;

  constructor(private readonly changed: () => void) {}

  entries(): CardEntry[] { return [...this.dialogs.values()].map(dialog => dialog.entry); }
  isExpired(id: string): boolean { return this.expired.has(id); }

  context(base: ExtensionUIContext, notify: ExtensionUIContext["notify"]): ExtensionUIContext {
    return {
      ...base, notify,
      select: (title, available, opts) => {
        const labels = available.slice(0, CARD_OPTION_MAX);
        const options = cardOptions(labels.map(label => ({ label })));
        return this.open<string | undefined>({ kind: "select", title, questions: [cardQuestion(title, options)],
          ...(available.length > labels.length ? { body: `Showing the first ${labels.length} of ${available.length} choices.` } : {}) },
        opts, undefined, answer => {
          if (answer.options.length !== 1 || answer.text !== undefined) invalidAnswer("select one option");
          const selected = labels.find((label, i) => optionId(label, i) === answer.options[0]);
          if (selected === undefined) invalidAnswer("select a listed option");
          return selected;
        });
      },
      confirm: (title, body, opts) => this.open({ kind: "confirm", title, body, questions: [cardQuestion(title, confirmOptions)] },
        opts, false, answer => {
          const choice = answer.options[0];
          if (answer.options.length !== 1 || (choice !== "yes" && choice !== "no") || answer.text !== undefined) {
            invalidAnswer("choose yes or no");
          }
          return choice === "yes";
        }),
      input: (title, placeholder, opts) => this.open<string | undefined>({ kind: "input", title, body: placeholder,
        questions: [cardQuestion(title, [], true)] }, opts, undefined, answer => {
        if (answer.options.length || answer.text === undefined) invalidAnswer("enter text");
        return answer.text;
      }),
    };
  }

  private open<T>(data: Pick<CardEntry["card"], "title" | "questions" | "body"> & { kind: "select" | "confirm" | "input" },
    opts: DialogOptions, dismissed: T, getValue: (answer: CardAnswer) => T): Promise<T> {
    const { signal, timeout } = opts ?? {};
    if (this.closed || signal?.aborted) return Promise.resolve(dismissed);
    const id = `dialog-${crypto.randomUUID()}`, now = Date.now();
    const expires = timeout !== undefined && Number.isFinite(timeout) && timeout > 0 ? now + Math.min(timeout, 8.64e15 - now) : undefined;
    const { promise, resolve } = Promise.withResolvers<T>();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = (value: T, expired = false) => {
      if (!this.dialogs.delete(id)) return;
      clearTimeout(timer);
      signal?.removeEventListener("abort", expire);
      if (expired) {
        this.expired.add(id);
        if (this.expired.size > RECENT_EXPIRED) this.expired.delete(this.expired.values().next().value!);
      }
      resolve(value);
      this.changed();
    };
    const expire = () => finish(dismissed, true);
    const schedule = () => {
      if (expires === undefined) return;
      const wait = expires - Date.now();
      if (wait <= 0) expire();
      else {
        timer = setTimeout(schedule, Math.min(wait, TIMEOUT_MAX));
        timer.unref();
      }
    };
    this.dialogs.set(id, { close: expire, entry: {
      card: { ...data, id, opened_at: new Date(now).toISOString(), expires_at: expires === undefined ? undefined : new Date(expires).toISOString() },
      answer: (answers, dismiss) => {
        if (!this.dialogs.has(id)) return this.expired.has(id) ? "expired" : "missing";
        if (signal?.aborted || expires !== undefined && expires <= Date.now()) { expire(); return "expired"; }
        const value = dismiss ? dismissed : getValue(singleAnswer(answers));
        finish(value);
        return dismiss ? "dismissed" : "answered";
      },
    } });
    signal?.addEventListener("abort", expire, { once: true });
    schedule();
    this.changed();
    return promise;
  }

  close(): void {
    this.closed = true;
    for (const dialog of this.dialogs.values()) dialog.close();
  }
}
