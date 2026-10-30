import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { gates, interventions, uniqueEvents } from "../src/events.ts";
import { measure, selectedRun } from "../src/metrics.ts";
import { canonical, parseEvents, sha256, showSchema } from "../src/schema.ts";
import type { Event } from "../src/schema.ts";

const show = showSchema.parse(JSON.parse(await readFile(new URL("./fixtures/show.json", import.meta.url), "utf8")));
const events = parseEvents(await readFile(new URL("./fixtures/events.jsonl", import.meta.url), "utf8"));
const event = (kind: string, second: number, detail: Readonly<Record<string, unknown>>): Event => ({
  ts: `2026-10-24T00:00:${String(second).padStart(2, "0")}Z`,
  task: "T-synthetic", kind, detail,
});

describe("raw metrics", () => {
  test("prices worker plus independent review using API-equivalent cost, not reported billing", () => {
    // Given the committed raw subscription fixture.
    const run = selectedRun(show, "R1");
    // When canonical readbacks are measured.
    const actual = measure(show, run, events);
    // Then known tokens, timing and separate billing are retained.
    expect(actual.api_equivalent_usd).toBe(1.2);
    expect(actual.subscription_reported_usd).toBe(0);
    expect(actual.tokens).toEqual({ input: 140, output: 30, cache_read: 35, cache_write: 10 });
    expect(actual.total_tokens).toBe(215);
    expect(actual.wall_seconds).toBe(60);
    expect(actual.gate_seconds).toBe(10);
  });

  test("counts idempotent needs-me events once and closes the exact decision", () => {
    // Given a duplicate request and a reply before its decision receipt.
    const run = selectedRun(show, "R1");
    // When the run is measured.
    const actual = measure(show, run, events);
    // Then the reply does not prematurely stop decision wait time.
    expect(actual.needs_me).toBe(1);
    expect(actual.needs_me_seconds).toBe(5);
    expect(actual.rescues).toBe(1);
    expect(actual.pm_answers).toBe(1);
    expect(actual.owner_answers).toBe(1);
    expect(actual.review_rounds).toBe(1);
  });

  test("isolates another run and uses native canonical silent-turn classes", () => {
    // Given both runs in the same raw task timeline.
    const run = selectedRun(show, "R2");
    // When the Pi run is measured.
    const actual = measure(show, run, events);
    // Then neither prior-run events nor another turn's completion hide the silent turn.
    expect(actual.needs_me).toBe(1);
    expect(actual.turns_ended_without_report).toBe(1);
    expect(actual.no_report_source).toBe("canonical_turn_class");
  });

  test("does not invent zero cost for unpriced usage", () => {
    // Given unknown pricing on the worker.
    const run = selectedRun(show, "R1");
    if (!run.usage) throw new TypeError("fixture usage missing");
    // When the unknown model is measured.
    const actual = measure(show, { ...run, usage: { ...run.usage, estimated_usd: null, unpriced_tokens: 100 } }, events);
    // Then the cost remains unavailable rather than subscription billing or zero.
    expect(actual.api_equivalent_usd).toBeNull();
    expect(actual.unpriced_tokens).toBe(100);
  });

  test("refuses missing independent reviewer usage", () => {
    // Given a run whose independent reviewer has no usage evidence.
    const run = selectedRun(show, "R1");
    // When measurement is attempted.
    const action = () => measure(show, { ...run, review_runs: run.review_runs.map((child) => ({ ...child, usage: null })) }, events);
    // Then incompleteness is explicit.
    expect(action).toThrow("usage_missing");
  });

  test("refuses evicted native turn history", () => {
    // Given a bounded native table beginning after turn one.
    const run = selectedRun(show, "R2");
    // When measurement is attempted.
    const action = () => measure(show, { ...run, agent_host: { turns: [{ generation: 1, seq: 5, class: "completed", ended: "2026-10-24T01:00:35Z" }] } }, events);
    // Then the missing silent turns cannot turn into a clean count.
    expect(action).toThrow("turn_history_truncated");
  });

  test("accepts native admission before its first canonical turn", () => {
    // Given a canonical admission that omits the empty turn table.
    const input = { ...show, runs: [{ ...selectedRun(show, "R2"), agent_host: {} }] };
    // When the raw readback is parsed.
    const admitted = showSchema.parse(input);
    // Then the empty table remains empty rather than blocking admission capture.
    expect(selectedRun(admitted, "R2").agent_host?.turns).toEqual([]);
  });

  test("refuses approved native measurement with an empty turn table", () => {
    // Given an approved native run without any canonical turn evidence.
    const run = { ...selectedRun(show, "R2"), agent_host: { turns: [] } };
    // When measurement is attempted.
    const action = () => measure(show, run, events);
    // Then accepting empty admission data cannot fabricate a clean completed run.
    expect(action).toThrow("turn_history_missing");
  });

  test("rejects a missing approval instead of treating done as completion", () => {
    // Given a done run without a verdict event.
    const filtered = events.filter((entry) => entry.kind !== "verdict");
    // When measurement is attempted.
    const action = () => measure(show, selectedRun(show, "R1"), filtered);
    // Then wall time to approval is not fabricated.
    expect(action).toThrow("approval_missing");
  });

  test("includes repair work through the latest approval after a failed landing", () => {
    // Given an approved run reopened by a failed landing and approved again.
    const run = selectedRun(show, "R1");
    const child = run.review_runs[0];
    if (!child) throw new TypeError("fixture reviewer missing");
    const repaired = { ...run, review_runs: [...run.review_runs, { ...child, id: "R1-review2" }] };
    const later = (kind: string, second: number, detail: Readonly<Record<string, unknown>>): Event => ({
      ts: new Date(Date.parse(run.started) + second * 1000).toISOString(),
      task: show.id, kind, detail: { run: run.id, ...detail },
    });
    const input = [...events,
      later("gate_started", 65, { identity: { attempt: "landing" }, tier: "full" }),
      later("gate", 75, { identity: { attempt: "landing" }, result: "fail" }),
      later("changes_requested", 76, {}),
      later("gate_started", 80, { identity: { attempt: "repair" }, tier: "full" }),
      later("gate", 90, { identity: { attempt: "repair" }, result: "pass" }),
      { ...later("commented", 95, { comment: { text: '{"comparison_rescue":"pane_input"}' } }), actor: "pm" },
      later("verdict", 120, { verdict: "approve", reviewer: "independent" }),
    ];
    // When the repaired run is measured, the first approval is not the endpoint.
    const actual = measure(show, repaired, input);
    expect(actual.approved_at).toBe(later("verdict", 120, {}).ts);
    expect(actual.wall_seconds).toBe(120);
    expect(actual.review_rounds).toBe(2);
    expect(actual.gate_count).toBe(3);
    expect(actual.gates_passed).toBe(2);
    expect(actual.rescues).toBe(2);
  });
});

describe("canonical event surfaces", () => {
  test("closes native question waits only after matching questions are answered", () => {
    // Given overlapping question holds, an unrelated answer and a plain reply.
    const input = [
      event("needs_me", 1, { question: "Q1" }),
      event("question_answered", 2, { key: "other", revision: 3, at: "2026-10-24T00:00:02Z", by: "owner" }),
      event("replied", 3, { submitted: true }),
      event("needs_me", 4, { question: "Q2" }),
      event("question_answered", 5, { key: "Q1", revision: 3, at: "2026-10-24T00:00:05Z", by: "owner" }),
      event("question_answered", 8, { key: "Q2", revision: 3, at: "2026-10-24T00:00:08Z", by: "owner" }),
      event("done", 20, {}),
    ];
    // When the canonical timeline is measured.
    const actual = interventions(uniqueEvents(input), Date.parse("2026-10-24T00:01:00Z"));
    // Then the wait ends at the last matching answer, not the reply or done.
    expect(actual.needs_me_seconds).toBe(7);
  });

  test.each(["question", "decision"])("keeps a pending %s hold after the other answer", (last) => {
    // Given an overlapping legacy decision and native question.
    const question = event("question_answered", last === "question" ? 8 : 5, {
      key: "Q1", revision: 3, at: `2026-10-24T00:00:0${last === "question" ? 8 : 5}Z`, by: "owner",
    });
    const decision = event("run_resumed", last === "decision" ? 8 : 5, { decision: "D1", outcome: "answered" });
    const input = [
      event("needs_me", 1, { decision: "D1" }),
      event("needs_me", 2, { question: "Q1" }),
      question, decision, event("done", 20, {}),
    ];
    // When both independently keyed holds are measured.
    const actual = interventions(uniqueEvents(input), Date.parse("2026-10-24T00:01:00Z"));
    // Then neither answer prematurely closes the other hold.
    expect(actual.needs_me_seconds).toBe(7);
  });

  test("raw export and log-all timeline wrappers produce identical measurements", () => {
    // Given raw events and the CLI timeline wrappers carrying those same raw events.
    const wrapped = events.map((entry) => JSON.stringify({ at: entry.ts, event: entry })).join("\n");
    // When both declared surfaces are read.
    const actual = measure(show, selectedRun(show, "R1"), parseEvents(wrapped));
    // Then the projection is equal, including gates and interventions.
    expect(actual).toEqual(measure(show, selectedRun(show, "R1"), parseEvents(JSON.stringify(events))));
  });

  test("legacy, tiered and native gate events share durations and stale classifications", () => {
    // Given three attempts with the canonical stale-result marker.
    const rows = [
      [event("gate_started", 1, { run: "R1" }), event("gate", 4, { run: "R1", result: "fail", failed_cmd: "stale_result: changed" })],
      [event("gate_started", 1, { run: "R1", tier: "quick", identity: { attempt: "tiered" } }), event("gate", 4, { run: "R1", result: "fail", failed_cmd: "stale_result: changed", identity: { attempt: "tiered" } })],
      [event("gate_started", 1, { run: "R1", tier: "quick", identity: { attempt: "ag-native" } }), event("gate", 4, { run: "R1", result: "fail", failed_cmd: "stale_result: changed", identity: { attempt: "ag-native" } })],
    ];
    // When each surface is paired.
    const actual = rows.map((input) => gates(input).map(({ seconds, result }) => ({ seconds, result })));
    // Then identities and tier presence do not change measurement.
    expect(actual).toEqual(Array.from({ length: 3 }, () => [{ seconds: 3, result: "stale_result" }]));
  });

  test("refuses unmatched gates", () => {
    // Given a terminal gate without a captured start.
    const input = [event("gate", 4, { run: "R1", result: "pass" })];
    // When pairing is attempted.
    const action = () => gates(input);
    // Then missing raw evidence is not a zero-duration success.
    expect(action).toThrow("gate_unmatched");
  });

  test("counts manual inbox rescues but not terminal no-ops or failed replies", () => {
    // Given actual inbox audit event names and a foreign recovery signal.
    const input = [
      event("message_retried", 1, { key: "T-synthetic_R1" }),
      event("inbox_recovered", 2, { via: "manual" }),
      event("message_abandoned", 3, { via: "manual" }),
      event("message_terminal_noop", 4, { via: "manual" }),
      event("replied", 5, { submitted: false }),
      event("replied", 6, { control: true }),
      event("message_observed", 7, { receipt: { via: "manual" } }),
      { ...event("commented", 8, { comment: { text: '{"comparison_rescue":"pane_input"}' } }), actor: "pm" },
      { ...event("commented", 9, { comment: { text: '{"comparison_rescue":"prerequisite_context"}' } }), actor: "pm" },
    ];
    // When interventions are counted.
    const actual = interventions(uniqueEvents(input), Date.parse("2026-10-24T00:01:00Z"));
    // Then only effective operator actions count.
    expect(actual.rescues).toBe(6);
  });

  test("canonical digest ignores JSON object key order", () => {
    // Given equivalent contracts serialized in different key orders.
    const left = { tier: "quick", commands: ["fixture-check"] };
    // When canonical control digests are computed.
    const actual = sha256(canonical(left));
    // Then the capture order does not invalidate a pair.
    expect(actual).toBe("1a38314356bc4e4b9307802d1cf67aa055d8f028ab96ae43a98d72831246100c");
  });

  test("whole-store export ignores unscoped PM and configuration events", () => {
    // Given task events mixed with an unscoped canonical store event.
    const raw = [...events, { ts: "2026-10-24T00:00:00Z", task: null,
      kind: "pm_opened", detail: { profile: "omo-pm" } }];
    // When the whole-store raw export is parsed.
    const actual = measure(show, selectedRun(show, "R1"), parseEvents(JSON.stringify(raw)));
    // Then PM data is not attributed to the task.
    expect(actual).toEqual(measure(show, selectedRun(show, "R1"), events));
  });
});
