import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { codexFailureCopy } from "../host/codex-failure-copy.ts";
import { ErrorRows, clockTime, errorCopy, errorPresenter, presentCodexError, presentError, type ErrorInput, type StopCause } from "./error-copy.ts";
import { PANEL_ERROR } from "./panels-view.ts";

const base: ErrorInput = { stopReason: "error", partial: false, toolCallsNotRun: 0, retryAttempts: 0 };
const rows = (input: ErrorInput, width: number) => presentError(input).render(width).map(line => stripTerminalSequences(line));

// Real error texts from session files.
const CODEX = "Codex error: An error occurred while processing your request. You can retry your request, or contact us through our "
  + "help center at help.openai.com if the error persists. Please include the request ID e8405ef7-bfee-4427-a8ca-d47ff4e35f7c in your message.";
const CODEX_UNKNOWN = "openai-subscription: unknown (unknown)";
const typedCodex = { version: 1, connection: "openai-subscription", category: "unknown", scope: "unknown" };
const safeRef = "8f3a1c2d".repeat(8);
const cases: readonly { readonly input: ErrorInput; readonly facts: readonly string[] }[] = [
  { input: { ...base, errorMessage: CODEX_UNKNOWN }, facts: ["OpenAI subscription", "No reply", "reason could not be determined"] },
  { input: { ...base, errorMessage: "budget_exhausted: the Claude agent reached its budget" }, facts: ["budget limit", "Nothing was sent", "another model"] },
  { input: { ...base, errorMessage: "anthropic-api: local_budget (request)" }, facts: ["budget limit", "Nothing was sent"] },
  { input: { ...base, errorMessage: CODEX }, facts: ["e8405ef7", "retry"] },
];

test("each known failure is a sentence and a next step, without its raw code", () => {
  for (const { input, facts } of cases) {
    for (const width of [100, 46]) {
      const shown = rows(input, width);
      expect(shown[0]!.startsWith(" ✗ ")).toBeTrue();
      expect(shown.length).toBeGreaterThanOrEqual(2);
      for (const row of shown) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
      const text = shown.map(line => line.trim()).join(" ");
      expect(text).not.toMatch(/[{}]|"code"|call_failed|call_timeout|\(unknown\)|Conflict/);
      if (width === 100) for (const fact of facts) expect(text).toContain(fact);
    }
  }
});

test("typed Codex failures report observed output and an actual replay without exposing raw diagnostics", () => {
  const show = (providerFailure: unknown) => {
    const view = new ErrorRows(errorCopy({ ...base, errorMessage: "PRIVATE_PROVIDER_BODY", providerFailure }));
    view.setExpanded(true);
    return view.render(120).map(line => stripTerminalSequences(line)).join(" ");
  };
  const unknown = show({ ...typedCodex, output_observed: true, request_ref: safeRef });
  expect(unknown).toContain("ref 8f3a1c2d");
  expect(unknown).toContain("response stopped early");
  expect(unknown).not.toContain("after one retry");
  expect(unknown).not.toContain(safeRef);
  expect(unknown).not.toContain("PRIVATE_PROVIDER_BODY");

  const replayed = show({ ...typedCodex, category: "auth", replayed: true, request_ref: "PRIVATE_REQUEST_ID" });
  expect(replayed).toContain("after one retry");
  expect(replayed).toContain("No reply");
  expect(replayed).toContain("Sign in again");
  expect(replayed).not.toContain("reason could not be determined");
  expect(replayed).not.toContain("PRIVATE");
  expect(replayed).not.toContain("(ref ");
});

test("an automatic retry-end does not repeat the assistant's Codex failure in either interactive mode", () => {
  const { sentence, step } = codexFailureCopy({ partial: false, retried: true, requestRef: safeRef, unknown: true });
  const retryEnded = { ...base, retryEnded: true, retryAttempts: 1 };
  const presentWithPanels = errorPresenter(() => undefined);
  for (const present of [presentWithPanels, presentCodexError]) {
    for (const errorMessage of [CODEX_UNKNOWN, `${sentence} ${step}`]) {
      expect(present({ ...base, errorMessage }) instanceof ErrorRows).toBeTrue();
      expect(present({ ...retryEnded, errorMessage })).toEqual([]);
    }
  }
  expect(presentWithPanels({ ...retryEnded, stopReason: "aborted", errorMessage: "Retry cancelled" }) instanceof ErrorRows).toBeTrue();
  expect(presentCodexError({ ...retryEnded, errorMessage: "anthropic-api: transient (unknown)" })).toBeUndefined();
  expect(presentWithPanels({ ...base, toolCallsNotRun: 1, errorMessage: CODEX_UNKNOWN }) instanceof ErrorRows).toBeTrue();
  expect(presentWithPanels({ ...base, toolCallsNotRun: 1, errorMessage: "anthropic-api: transient (unknown)" })).toBeUndefined();
});

test("a reset time from the provider appears as a clock time", () => {
  const now = Date.parse("2026-11-13T10:00:00Z"), reset = Date.parse("2026-11-13T12:30:00Z");
  const copy = errorCopy({ ...base, providerFailure: { version: 1, connection: "openai-subscription", category: "subscription_quota",
    scope: "account", reset_at: new Date(reset).toISOString() } }, now);
  expect(copy.step ?? "").toContain(clockTime(reset, now));
  const after = errorCopy({ ...base, timestamp: now, providerFailure: { version: 1, connection: "anthropic-api", category: "transient",
    scope: "unknown", retry_after_secs: 120 } }, now);
  expect(after.step ?? "").toContain(clockTime(now + 120_000, now));
});

test("expanding shows the raw text the collapsed rows leave out", () => {
  const view = new ErrorRows(errorCopy({ ...base, errorMessage: CODEX }));
  const collapsed = view.render(100).length;
  view.setExpanded(true);
  const open = view.render(100).map(line => stripTerminalSequences(line));
  expect(open.length).toBeGreaterThan(collapsed);
  expect(open.join(" ")).toContain("help.openai.com");
});

test("expanded shows the whole raw text, however long", () => {
  const long = `${Array.from({ length: 120 }, (_, i) => `diagnostic line ${i}`).join("\n")}\nrequest id tail-7f3e`;
  const view = new ErrorRows(errorCopy({ ...base, errorMessage: long }));
  view.setExpanded(true);
  const open = view.render(46).map(line => stripTerminalSequences(line)).join("\n");
  expect(open).toContain("request id tail-7f3e");
  expect(open).not.toMatch(/more rows?$/m);
});

test("an unknown error keeps its text after the glyph, clipped by rows with a count", () => {
  const text = Array.from({ length: 30 }, (_, i) => `word${i}`).join(" ");
  const shown = rows({ ...base, errorMessage: text }, 46);
  expect(shown[0]!.startsWith(" ✗ word0")).toBeTrue();
  expect(shown.some(row => /more rows?$/.test(row.trim()))).toBeTrue();
  expect(shown.join(" ")).not.toContain("word29");
});

test("a stopped turn names who stopped it only when the host knows, never in the error colour", () => {
  const stop: ErrorInput = { ...base, stopReason: "aborted", errorMessage: "Operation aborted", toolCallsNotRun: 2 };
  const cases: readonly [StopCause | undefined, RegExp][] = [
    ["owner", /^ ○ stopped by you · 2 tool calls did not run/],
    ["host", /^ ○ stopped by corral · 2 tool calls did not run/],
    [undefined, /^ ○ stopped · 2 tool calls did not run/],
  ];
  for (const [cause, shape] of cases) {
    const shown = presentError(stop, cause).render(100);
    expect(shown).toHaveLength(1);
    expect(stripTerminalSequences(shown[0]!)).toMatch(shape);
    expect(shown[0]).not.toContain("✗");
    expect(shown[0]).not.toContain(PANEL_ERROR);
  }
});

test("the host's stop cause applies to a stop drawn live, and the same turn drawn again keeps it", () => {
  let latest: StopCause | undefined = "owner";
  const present = errorPresenter(() => latest);
  const stop: ErrorInput = { ...base, stopReason: "aborted", errorMessage: "Operation aborted", timestamp: 1_000 };
  const shown = (input: ErrorInput) => {
    const view = present(input);
    if (!view || Array.isArray(view)) throw new Error("stop expected to have a row");
    return stripTerminalSequences(view.render(100)[0]!);
  };
  expect(shown({ ...stop, live: true })).toContain("stopped by you");
  latest = undefined;
  expect(shown(stop)).toContain("stopped by you");
  // An older stop drawn again on resume: the latest run's cause does not apply.
  latest = "owner";
  expect(shown({ ...stop, timestamp: 500 })).not.toContain("by you");
});
