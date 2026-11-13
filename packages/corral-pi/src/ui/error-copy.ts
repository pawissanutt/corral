// The words a failed or stopped turn reads as in the transcript. Pi hands every assistant message that ended in an
// error or a stop, and the end of automatic retrying, to `presentError` (the `errorPresentation` option of the patched
// InteractiveMode). A failure becomes one `✗` sentence saying what happened and whether anything was lost, then one
// muted next step; the raw text and the typed failure facts are one click or Ctrl+O away. A stopped turn is one muted
// `○ stopped` row that names who stopped it when the host knows (`stopped by you`, `stopped: the run budget was
// reached`). Presentation only: the model and the journal receive what they did before.
import type { InteractiveModeOptions } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, wrapTextWithAnsi, type Component, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import type { ProviderFailure } from "../host/provider-failure.ts";
import { PANEL_ERROR, PANEL_MUTED, PANEL_TEXT, RESET, plainLabel } from "./panels-view.ts";

/** What Pi tells the hook about a failed or stopped turn. */
export type ErrorInput = Parameters<NonNullable<InteractiveModeOptions["errorPresentation"]>>[0];

/** The copy of one failed or stopped turn. */
export interface ErrorCopy {
  /** `failed`: a `✗` sentence and a step; `stopped`: one muted `○` row. */
  readonly kind: "failed" | "stopped";
  readonly sentence: string;
  readonly step?: string;
  /** The raw text and failure facts, shown expanded. */
  readonly raw: readonly string[];
  /** The sentence is the raw error text itself (nothing recognised it): clipped by rows with a count. */
  readonly verbatim?: boolean;
}

const CATEGORIES: Readonly<Record<ProviderFailure["category"], true>> = {
  subscription_quota: true, transient: true, auth: true, billing: true, invalid_request: true, unavailable: true,
  unknown: true, local_budget: true, network_policy: true,
};
const SCOPES: Readonly<Record<ProviderFailure["scope"], true>> = { account: true, model: true, request: true, unknown: true };
const CONNECTION_NAMES: Readonly<Record<string, string>> = {
  "openai-subscription": "OpenAI subscription", "openai-api": "OpenAI API", "anthropic-subscription": "Claude subscription",
  "anthropic-api": "Anthropic API", "amazon-bedrock": "Amazon Bedrock", "llama.cpp": "llama.cpp",
};
/** The texts Pi and its providers give a turn the owner stopped. */
const STOP_TEXTS: Readonly<Record<string, true>> = {
  "Operation aborted": true, "Request was aborted": true, "The operation was aborted.": true, "Retry cancelled": true,
};
const SUMMARY = /^(?:(claude_compaction_conflict|claude_query_incomplete|search turn limit): )?([a-z0-9.-]+): ([a-z_]+) \(([a-z]+)\)$/;
const RETRY_STEP = "Send a message to retry.";
const RETRY_OR_MODEL = "Send a message to retry, or choose another model.";
const SENTENCE_ROWS = 4;
const VERBATIM_ROWS = 2;
const STEP_ROWS = 3;

function failureOf(value: unknown): ProviderFailure | undefined {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return undefined;
  const v = value as Record<string, unknown>;
  if (v.version !== 1 || typeof v.connection !== "string" || typeof v.category !== "string" || typeof v.scope !== "string") return undefined;
  if (!Object.hasOwn(CATEGORIES, v.category) || !Object.hasOwn(SCOPES, v.scope)) return undefined;
  return value as ProviderFailure;
}

/** `14:20` today, `Tue 14:20` within the week, `Nov 20 14:20` beyond, in local time. */
export function clockTime(at: number, now: number): string {
  const date = new Date(at), today = new Date(now);
  const hm = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
  if (date.toDateString() === today.toDateString()) return hm;
  const days = Math.abs(at - now) / 86_400_000;
  return days < 6 ? `${date.toLocaleDateString("en-US", { weekday: "short" })} ${hm}`
    : `${date.toLocaleDateString("en-US", { month: "short", day: "numeric" })} ${hm}`;
}

function lostClause(input: ErrorInput, sent: boolean): string {
  return input.partial ? "The reply above stopped early." : sent ? "Nothing was lost." : "Nothing was sent to the model.";
}
function retryCount(n: number): string {
  return `${n} automatic ${n === 1 ? "retry" : "retries"}`;
}

/** When the provider said the request may be sent again, as a clock time, or undefined. */
function waitUntil(failure: ProviderFailure, input: ErrorInput, now: number): string | undefined {
  const reset = failure.reset_at ? Date.parse(failure.reset_at) : Number.NaN;
  if (Number.isFinite(reset)) return clockTime(reset, now);
  if (failure.retry_after_secs !== undefined) return clockTime((input.timestamp ?? now) + failure.retry_after_secs * 1000, now);
  return undefined;
}

function providerCopy(failure: ProviderFailure, input: ErrorInput, now: number, raw: readonly string[]): ErrorCopy {
  const name = CONNECTION_NAMES[failure.connection] ?? failure.connection;
  const ref = failure.request_ref ? ` (ref ${failure.request_ref.slice(0, 8)})` : "", until = waitUntil(failure, input, now);
  const failed = (sentence: string, step: string): ErrorCopy => ({ kind: "failed", sentence, step, raw });
  switch (failure.category) {
    case "transient":
      return failed(`${name} is overloaded or limiting requests${ref}. ${lostClause(input, true)}`,
        until ? `Wait until ${until}, then send a message to retry, or choose another model.` : "Send a message to retry in a moment, or choose another model.");
    case "subscription_quota":
      return failed(`The ${name} usage limit${failure.scope === "model" ? " for this model" : ""} is reached. ${lostClause(input, true)}`,
        until ? `It resets at ${until}: wait until then, or choose another model.` : "Wait for the limit to reset, or choose another model.");
    case "auth":
      return failed(`${name} refused the sign-in for this account. ${lostClause(input, true)}`, "Sign in again, or choose another model.");
    case "billing":
      return failed(`${name} refused the request: the account has no credit or plan for it. ${lostClause(input, true)}`,
        "Check the account's billing, or choose another model.");
    case "invalid_request":
      return failed(`${name} rejected the request as invalid${ref}. ${lostClause(input, true)}`,
        "Sending it again fails the same way: compact the conversation, rephrase, or choose another model.");
    case "unavailable":
      return failed(`This model is not available on the ${name}. ${lostClause(input, true)}`, "Choose another model.");
    case "local_budget":
      return failed(`The run's budget is spent, so ${input.partial ? "the reply above stopped early" : "the request was not sent to the model"}.`,
        "The agent cannot continue until the budget limits are raised.");
    case "network_policy":
      return failed(`This session's network policy blocked the request. ${lostClause(input, false)}`,
        "The destination is not allowed here; retrying is blocked the same way.");
    case "unknown":
      return failed(`The ${name} request failed and gave no reason${ref}. ${lostClause(input, true)}`, RETRY_OR_MODEL);
  }
}

/** A `budget record refused: {...}` refusal: the code and reason corral gave, or undefined when not that shape. */
function refusalOf(text: string): { readonly code: string; readonly reason: string } | undefined {
  const match = /^budget record refused: (\{.*\})$/s.exec(text);
  if (!match) return undefined;
  try {
    const parsed: unknown = JSON.parse(match[1]!);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return undefined;
    const record = parsed as Record<string, unknown>;
    return typeof record.code === "string" ? { code: record.code, reason: typeof record.reason === "string" ? record.reason : "" } : undefined;
  } catch { return undefined; }
}

function hostCopy(text: string, input: ErrorInput, raw: readonly string[]): ErrorCopy | undefined {
  const failed = (sentence: string, step: string): ErrorCopy => ({ kind: "failed", sentence, step, raw });
  const notSent = lostClause(input, false);
  const refusal = refusalOf(text);
  if (refusal) {
    if (refusal.code === "receipt_limit") {
      const count = /holds (\d+) receipts/.exec(refusal.reason)?.[1];
      const held = count ? `already holds ${Number(count).toLocaleString("en-US")} receipts` : "is full";
      return failed(`Corral refused this request: the task's record ${held}. ${notSent}`,
        "The agent cannot continue until the receipt limit is dealt with; restarting hits the same limit.");
    }
    if (refusal.code === "review_closed") {
      return failed(`The review this session belongs to is closed, so corral refused the request. ${notSent}`, "This review session cannot continue.");
    }
    if (refusal.code === "run_ended") {
      return failed(`The run this session belongs to has ended, so corral refused the request. ${notSent}`, "This run's session cannot continue.");
    }
    return failed(`Corral refused to record this request (${plainLabel(refusal.code).replace(/_/g, " ")}). ${notSent}`, RETRY_STEP);
  }
  if (text.startsWith("call_failed:")) {
    if (/\bConflict \{/.test(text)) return failed(`Another corral write landed first, so this request was not recorded. ${notSent}`, RETRY_STEP);
    if (/database is locked/.test(text)) return failed(`Corral's database was busy, so this request was not recorded. ${notSent}`, RETRY_STEP);
    if (/signal=SIG[A-Z]+/.test(text)) return failed(`The corral call recording this request was interrupted. ${notSent}`, RETRY_STEP);
    return failed(`A corral call recording this request failed. ${notSent}`, RETRY_STEP);
  }
  if (text.startsWith("call_timeout:")) return failed(`Corral did not answer in time while recording this request. ${notSent}`, RETRY_STEP);
  if (text.startsWith("budget_accounting_refused:")) return failed(`Corral could not record the cost of this request. ${notSent}`, RETRY_STEP);
  if (text.startsWith("cache_guard_paused:")) {
    return failed(`The PM's cache-health hold paused model requests. ${notSent}`, "The owner resumes it with corral pm guard-resume.");
  }
  return undefined;
}

function transportCopy(text: string, input: ErrorInput, raw: readonly string[]): ErrorCopy | undefined {
  const failed = (sentence: string, step: string): ErrorCopy => ({ kind: "failed", sentence, step, raw });
  const lost = lostClause(input, true);
  const codex = /^Codex error: (.*)$/s.exec(text)?.[1];
  if (codex !== undefined) {
    if (/overloaded/i.test(codex)) return failed(`OpenAI is overloaded. ${lost}`, "Send a message to retry in a moment, or choose another model.");
    if (/flagged/i.test(codex)) return failed(`OpenAI refused the request: its content was flagged. ${lost}`, "Rephrase the request, or choose another model.");
    if (/error occurred while processing/i.test(codex)) {
      const id = /request ID ([0-9a-f-]{8,})/i.exec(codex)?.[1];
      return failed(`OpenAI could not process the request${id ? ` (request ${id.slice(0, 8)})` : ""}. ${lost}`, RETRY_OR_MODEL);
    }
    return undefined;
  }
  const blocked = /^All Claude accounts .* are currently blocked \(authentication error\)/.exec(text);
  if (blocked || text.startsWith("authentication_failed:") || text.startsWith("credentials_unavailable:")) {
    const check = /Next automatic re-check: (\S+?)\.?(?:\s|$)/.exec(text)?.[1];
    const at = check ? Date.parse(check) : Number.NaN;
    return failed(`The model provider refused the sign-in. ${lost}`,
      Number.isFinite(at) ? `Sign in again, or wait for the re-check at ${clockTime(at, Date.now())}.` : "Sign in again, or choose another model.");
  }
  if (text.startsWith("budget_exhausted:")) {
    return failed(`The run's budget is spent, so ${input.partial ? "the reply above stopped early" : "the request was not sent to the model"}.`,
      "The agent cannot continue until the budget limits are raised.");
  }
  if (text.startsWith("network_refused:")) {
    return failed(`This session's network policy blocked the request. ${lostClause(input, false)}`, "The destination is not allowed here; retrying is blocked the same way.");
  }
  const provider = /^Provider is not configured: (\S+)/.exec(text)?.[1];
  if (provider) return failed(`The provider ${plainLabel(provider)} is not set up for this session. ${lost}`, "Choose another model.");
  if (/socket connection was closed|WebSocket closed|ECONNRESET|getaddrinfo|ETIMEDOUT|ETIMEOUT|upstream connect error|liveness timeout|^The operation was aborted\.$/.test(text)) {
    return failed(`The connection to the model dropped before the reply finished. ${lost}`, RETRY_STEP);
  }
  if (/timed out after \d+ms/.test(text)) return failed(`The model did not start answering in time. ${lost}`, RETRY_STEP);
  if (/^\s*<(?:!doctype|html)/i.test(text)) return failed(`The provider answered with an error page instead of a reply. ${lost}`, RETRY_OR_MODEL);
  const status = /^(\d{3}) /.exec(text)?.[1];
  if (status) {
    const code = Number(status);
    if (code === 401 || code === 403) return failed(`The model provider refused the sign-in (HTTP ${code}). ${lost}`, "Sign in again, or choose another model.");
    if (code === 429 || code >= 500) return failed(`The model provider is overloaded or limiting requests (HTTP ${code}). ${lost}`, "Send a message to retry in a moment, or choose another model.");
    if (code === 404) return failed(`The model provider does not know this model (HTTP 404). ${lost}`, "Choose another model.");
    if (code >= 400) {
      return failed(`The model provider rejected the request (HTTP ${code}). ${lost}`,
        "Sending it again fails the same way: compact the conversation, rephrase, or choose another model.");
    }
  }
  return undefined;
}

function factsLine(failure: ProviderFailure): string {
  return [failure.connection, failure.category, failure.scope,
    ...(failure.retry_after_secs !== undefined ? [`retry after ${failure.retry_after_secs}s`] : []),
    ...(failure.reset_at ? [`resets ${failure.reset_at}`] : []),
    ...(failure.request_ref ? [`ref ${failure.request_ref}`] : [])].join(" · ");
}

/** Who stopped a turn, as the host knows it for its latest run. */
export type StopCause = "owner" | "budget" | "host";

/** The copy for one failed or stopped turn. `now` is for clock times; `cause` says who stopped it, when known. */
export function errorCopy(input: ErrorInput, now = Date.now(), cause?: StopCause): ErrorCopy {
  const text = input.errorMessage?.trim() ?? "";
  const failure = failureOf(input.providerFailure);
  const raw = [...(text ? [text] : []), ...(failure ? [factsLine(failure)] : [])];
  const retried = input.retryAttempts > 0 ? ` · after ${retryCount(input.retryAttempts)}` : "";
  if (input.stopReason === "aborted") {
    // Only the host's word makes it the owner's stop; a stop of an earlier run (resume, rebuild) has no known cause.
    const stopped = cause === "owner" ? "stopped by you"
      : cause === "budget" || /^budget_exhausted/.test(text) ? "stopped: the run budget was reached"
        : cause === "host" ? "stopped by corral" : "stopped";
    if (input.retryEnded) return { kind: "stopped", sentence: `${stopped}${retried}`, raw: [] };
    const notRun = input.toolCallsNotRun > 0
      ? ` · ${input.toolCallsNotRun} tool ${input.toolCallsNotRun === 1 ? "call" : "calls"} did not run` : "";
    const known = !text || Object.hasOwn(STOP_TEXTS, text) || /^Aborted after \d+ retry attempts?$/.test(text);
    return { kind: "stopped", sentence: `${stopped}${notRun}${retried}`, raw: known ? [] : [text] };
  }
  if (input.retryEnded) return { kind: "failed", sentence: `Gave up after ${retryCount(input.retryAttempts)}.`, raw };
  const summary = SUMMARY.exec(text);
  const summarized = summary && Object.hasOwn(CATEGORIES, summary[3]!) && Object.hasOwn(SCOPES, summary[4]!)
    ? { version: 1 as const, connection: summary[2]!, category: summary[3]! as ProviderFailure["category"], scope: summary[4]! as ProviderFailure["scope"] }
    : undefined;
  const typed = failure ?? (summarized as ProviderFailure | undefined);
  if (typed) return providerCopy(typed, input, now, raw);
  const known = hostCopy(text, input, raw) ?? transportCopy(text, input, raw);
  if (known) return known;
  return { kind: "failed", sentence: text ? plainLabel(text) : "The turn failed and gave no reason.", step: RETRY_OR_MODEL, raw, verbatim: text !== "" };
}

/** A failed or stopped turn as rows; click or Ctrl+O shows the raw text and failure facts under them. */
export class ErrorRows implements Component {
  private expanded = false;
  constructor(readonly copy: ErrorCopy) {}
  setExpanded(expanded: boolean): void { this.expanded = expanded; }
  invalidate(): void {}
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type !== "click" || event.button !== "left" || this.copy.raw.length === 0) return undefined;
    this.expanded = !this.expanded;
    return { handled: true };
  }
  render(width: number): string[] {
    // The row kit's geometry: one cell of padding each side, the glyph first, the words under the words.
    const pad = Math.max(0, Math.min(1, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2);
    const lead = " ".repeat(pad), copy = this.copy;
    if (copy.kind === "stopped") {
      return [lead + truncateToWidth(PANEL_MUTED + "○ " + copy.sentence, room, "…") + RESET, ...this.rawRows(lead, room)];
    }
    const inner = Math.max(1, room - 2), indent = lead + "  ";
    const sentence = wrapTextWithAnsi(copy.sentence, inner);
    const limit = copy.verbatim ? VERBATIM_ROWS : SENTENCE_ROWS;
    const shown = sentence.slice(0, limit);
    const cut = sentence.length - shown.length;
    if (cut > 0) shown[shown.length - 1] = truncateToWidth(shown[shown.length - 1]! + " …", inner, "…");
    const rows = shown.map((line, index) => (index === 0 ? lead + PANEL_ERROR + "✗ " : indent) + PANEL_TEXT + line + RESET);
    // Expanded, the whole raw text follows, so the count is only for the collapsed view.
    if (cut > 0 && copy.verbatim && !this.expanded) rows.push(indent + PANEL_MUTED + truncateToWidth(`… ${cut} more ${cut === 1 ? "row" : "rows"}`, inner, "…") + RESET);
    if (copy.step) {
      const step = wrapTextWithAnsi(copy.step, inner);
      const steps = step.slice(0, STEP_ROWS);
      if (step.length > steps.length) steps[steps.length - 1] = truncateToWidth(steps[steps.length - 1]! + " …", inner, "…");
      rows.push(...steps.map(line => indent + PANEL_MUTED + line + RESET));
    }
    return [...rows, ...this.rawRows(indent, inner)];
  }
  private rawRows(indent: string, inner: number): string[] {
    if (!this.expanded || this.copy.raw.length === 0) return [];
    // The whole raw text as plain rows, never cut: controls become spaces, tabs two spaces.
    return this.copy.raw.join("\n").replace(/\t/g, "  ").replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ")
      .split("\n").flatMap(line => line === "" ? [""] : wrapTextWithAnsi(line, inner))
      .map(line => indent + PANEL_MUTED + line + RESET);
  }
}

/** Corral's rows for one failed or stopped turn; `cause` says who stopped it, when known. */
export function presentError(input: ErrorInput, cause?: StopCause): Component {
  return new ErrorRows(errorCopy(input, Date.now(), cause));
}

/**
 * The `errorPresentation` hook for a host. A stop Pi draws as it happens (`live`) takes the host's latest stop cause,
 * remembered by the turn's time so the same turn drawn again in this process keeps it; a stop from an earlier
 * process has no known cause and reads `○ stopped`.
 */
export function errorPresenter(latestStopCause: () => StopCause | undefined): (input: ErrorInput) => Component {
  const causes = new Map<number, StopCause>();
  return input => {
    let cause = input.timestamp === undefined ? undefined : causes.get(input.timestamp);
    if (input.stopReason === "aborted" && input.live) {
      cause = latestStopCause();
      if (cause && input.timestamp !== undefined) causes.set(input.timestamp, cause);
    }
    return presentError(input, cause);
  };
}
