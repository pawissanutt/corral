import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { ErrorRows, clockTime, errorCopy, errorPresenter, presentError, type ErrorInput, type StopCause } from "./error-copy.ts";
import { PANEL_ERROR } from "./panels-view.ts";

const base: ErrorInput = { stopReason: "error", partial: false, toolCallsNotRun: 0, retryAttempts: 0 };
const rows = (input: ErrorInput, width: number) => presentError(input).render(width).map(line => stripTerminalSequences(line));

// Real error texts from session files.
const RECEIPTS = 'budget record refused: {"code":"receipt_limit","reason":"the container already holds 16384 receipts"}';
const CONFLICT = "call_failed: corral agent call --op event: exit=1, signal=null, timed_out=false; error: committing record unit: "
  + 'concurrent write refused: Conflict { subject: "record assistant-containers/A-77", expected: "Some(2089)", found: "Some(2090)" }';
const CODEX = "Codex error: An error occurred while processing your request. You can retry your request, or contact us through our "
  + "help center at help.openai.com if the error persists. Please include the request ID e8405ef7-bfee-4427-a8ca-d47ff4e35f7c in your message.";
const cases: readonly { readonly input: ErrorInput; readonly facts: readonly string[] }[] = [
  { input: { ...base, errorMessage: "openai-subscription: unknown (unknown)",
    providerFailure: { version: 1, connection: "openai-subscription", category: "unknown", scope: "unknown", request_ref: "8f3a1c2d9e" } },
  facts: ["OpenAI subscription", "8f3a1c2d", "Nothing was lost"] },
  { input: { ...base, errorMessage: RECEIPTS }, facts: ["16,384 receipts", "cannot continue"] },
  { input: { ...base, errorMessage: 'budget record refused: {"code":"review_closed","reason":"review child review_closed"}' }, facts: ["review", "closed"] },
  { input: { ...base, errorMessage: 'budget record refused: {"code":"run_ended","reason":"the named run no longer accepts work"}' }, facts: ["run", "ended"] },
  { input: { ...base, errorMessage: CONFLICT }, facts: ["not recorded", "retry"] },
  { input: { ...base, errorMessage: "call_timeout: corral agent call --op event: exit=null, signal=SIGKILL, timed_out=true; " }, facts: ["in time", "retry"] },
  { input: { ...base, errorMessage: CODEX }, facts: ["e8405ef7", "retry"] },
];

test("each known failure is a sentence and a next step, without its raw code", () => {
  for (const { input, facts } of cases) {
    for (const width of [100, 46]) {
      const shown = rows(input, width);
      expect(shown[0]!.startsWith(" ✗ ")).toBeTrue();
      expect(shown.length).toBeGreaterThanOrEqual(2);
      for (const row of shown) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
      const text = shown.join(" ");
      expect(text).not.toMatch(/[{}]|"code"|call_failed|call_timeout|\(unknown\)|Conflict/);
      if (width === 100) for (const fact of facts) expect(text).toContain(fact);
    }
  }
});

test("the receipt-limit copy says the agent cannot continue and promises no fix", () => {
  const copy = errorCopy({ ...base, errorMessage: RECEIPTS });
  expect(copy.step ?? "").toMatch(/cannot continue until the receipt limit/);
  expect(`${copy.sentence} ${copy.step ?? ""}`).not.toMatch(/fixed|will be|automatically/);
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
  const view = new ErrorRows(errorCopy({ ...base, errorMessage: RECEIPTS }));
  const collapsed = view.render(100).length;
  view.setExpanded(true);
  const open = view.render(100).map(line => stripTerminalSequences(line));
  expect(open.length).toBeGreaterThan(collapsed);
  expect(open.join(" ")).toContain('"code":"receipt_limit"');
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
    ["budget", /^ ○ stopped: the run budget was reached/],
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
  const shown = (input: ErrorInput) => stripTerminalSequences(present(input).render(100)[0]!);
  expect(shown({ ...stop, live: true })).toContain("stopped by you");
  latest = undefined;
  expect(shown(stop)).toContain("stopped by you");
  // An older stop drawn again on resume: the latest run's cause does not apply.
  latest = "owner";
  expect(shown({ ...stop, timestamp: 500 })).not.toContain("by you");
});
