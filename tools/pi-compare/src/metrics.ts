import { gates, interventions, uniqueEvents } from "./events.ts";
import { ComparisonError, text } from "./schema.ts";
import type { Event, Run, Show } from "./schema.ts";

export function selectedRun(show: Show, id: string): Run {
  const run = show.runs.find((candidate) => candidate.id === id);
  if (!run) throw new ComparisonError("run_missing", `${show.id}/${id}`);
  return run;
}

export function runEvents(events: readonly Event[], show: Show, run: Run): readonly Event[] {
  const start = Date.parse(run.started);
  const next = show.runs
    .filter((other) => Date.parse(other.started) > start)
    .map((other) => Date.parse(other.started)).sort((a, b) => a - b)[0] ?? Infinity;
  return uniqueEvents(events).filter((event) => {
    if (event.task !== show.id) return false;
    const named = text(event.detail, "run");
    const container = text(event.detail, "container");
    if (container && container !== `${show.id}/${run.id}`
      && !container.startsWith(`${show.id}/${run.id}/`)) return false;
    if (event.kind.startsWith("question_")) {
      const key = text(event.detail, "key");
      if (key && !key.startsWith(`${show.id}/${run.id}/`)) return false;
    }
    // Inbox events name a recipient instead of a run. Never charge another recipient.
    const recipient = event.kind.startsWith("message_") || event.kind.startsWith("inbox_")
      ? text(event.detail, "key") : undefined;
    if (recipient && recipient !== `${show.id}_${run.id}`
      && !recipient.startsWith(`${show.id}_${run.id}_rv-`)) return false;
    if (named && named !== run.id && !run.review_runs.some((child) => child.id === named)) return false;
    const at = Date.parse(event.ts);
    return at >= start && at < next;
  });
}

export function measure(show: Show, run: Run, input: readonly Event[]) {
  const events = runEvents(input, show, run);
  const dispatched = events.find((event) => event.kind === "dispatched"
    && text(event.detail, "run") === run.id);
  if (!dispatched) throw new ComparisonError("dispatch_missing", `${show.id}/${run.id}`);
  const approval = events.findLast((event) => event.kind === "verdict"
    && event.detail["verdict"] === "approve");
  if (!approval || run.review?.verdict !== "approve") {
    throw new ComparisonError("approval_missing", `${show.id}/${run.id}`);
  }
  const end = Date.parse(approval.ts);
  if (end < Date.parse(dispatched.ts)) throw new ComparisonError("negative_wall_time", run.id);
  const window = events.filter((event) => Date.parse(event.ts) <= end);
  const verdicts = window.filter((event) => event.kind === "verdict");
  if (!run.review_runs.length || verdicts.length !== run.review_runs.length) {
    throw new ComparisonError("independent_review_missing", `${show.id}/${run.id}`);
  }
  const usages = [run.usage, ...run.review_runs.map((child) => child.usage)];
  if (usages.some((usage) => !usage)) {
    throw new ComparisonError("usage_missing", `${show.id}/${run.id}`);
  }
  const priced = usages.every((usage) => usage?.estimated_usd != null && usage.unpriced_tokens === 0);
  const reported = usages.every((usage) => usage?.reported_usd != null);
  const sum = (key: "input" | "output" | "cache_read" | "cache_write" | "unpriced_tokens") =>
    usages.reduce((total, usage) => total + (usage?.[key] ?? 0), 0);
  const tokenFields = {
    input: sum("input"), output: sum("output"),
    cache_read: sum("cache_read"), cache_write: sum("cache_write"),
  };
  const hostRuns = [run, ...run.review_runs];
  // The bounded turn table cannot prove a whole-run count after eviction.
  for (const hostRun of hostRuns) {
    const firstByGeneration = new Map<number, number>();
    for (const turn of hostRun.agent_host?.turns ?? []) {
      firstByGeneration.set(turn.generation, Math.min(
        firstByGeneration.get(turn.generation) ?? Infinity, turn.seq,
      ));
    }
    if ([...firstByGeneration.values()].some((seq) => seq > 1)) {
      throw new ComparisonError("turn_history_truncated", `${show.id}/${hostRun.id}`);
    }
  }
  if (run.harness === "pi-native" && !run.agent_host) {
    throw new ComparisonError("turn_history_missing", `${show.id}/${run.id}`);
  }
  if (run.harness === "pi-native" && !run.agent_host?.turns.length) {
    throw new ComparisonError("turn_history_missing", `${show.id}/${run.id}`);
  }
  const noReport = run.harness === "pi-native"
    ? hostRuns.flatMap((item) => item.agent_host?.turns ?? []).filter((turn) =>
      turn.ended != null && Date.parse(turn.ended) <= end
        && (turn.class === "no_report" || turn.class === "no_verdict")).length
    : window.filter((event) => event.kind === "idle_no_report"
      || (event.kind === "review_escalated"
        && event.detail["reason"] === "idle_no_report")).length;
  const gateRows = gates(window);
  if (!gateRows.length) throw new ComparisonError("gate_missing", `${show.id}/${run.id}`);
  const passed = gateRows.filter((row) => row.result === "pass").length;
  return {
    task: show.id, run: run.id, harness: run.harness,
    approved_at: approval.ts,
    review_rounds: verdicts.length,
    ...interventions(window, end),
    turns_ended_without_report: noReport,
    no_report_source: run.harness === "pi-native" ? "canonical_turn_class" : "idle_no_report_event",
    gate_count: gateRows.length, gates_passed: passed,
    gate_pass_rate: passed / gateRows.length,
    gate_seconds: gateRows.reduce((total, row) => total + row.seconds, 0),
    gates: gateRows,
    carries: window.filter((event) => ["gate_carried", "verdict_carried", "evidence_carried"].includes(event.kind)).length,
    tokens: tokenFields,
    total_tokens: Object.values(tokenFields).reduce((a, b) => a + b, 0),
    api_equivalent_usd: priced
      ? usages.reduce((total, usage) => total + (usage?.estimated_usd ?? 0), 0) : null,
    subscription_reported_usd: reported
      ? usages.reduce((total, usage) => total + (usage?.reported_usd ?? 0), 0) : null,
    unpriced_tokens: sum("unpriced_tokens"),
    wall_seconds: (end - Date.parse(dispatched.ts)) / 1000,
  };
}
export type Metrics = ReturnType<typeof measure>;

export function aggregate(items: readonly Metrics[]) {
  const sum = (get: (item: Metrics) => number) => items.reduce((total, item) => total + get(item), 0);
  const priced = items.every((item) => item.api_equivalent_usd !== null);
  const finished = sum((item) => item.gate_count);
  return {
    runs: items.length,
    review_rounds: sum((item) => item.review_rounds),
    rescues: sum((item) => item.rescues), needs_me: sum((item) => item.needs_me),
    needs_me_seconds: sum((item) => item.needs_me_seconds),
    turns_ended_without_report: sum((item) => item.turns_ended_without_report),
    pm_answers: sum((item) => item.pm_answers), owner_answers: sum((item) => item.owner_answers),
    gate_count: finished, gates_passed: sum((item) => item.gates_passed),
    gate_pass_rate: finished ? sum((item) => item.gates_passed) / finished : null,
    gate_seconds: sum((item) => item.gate_seconds),
    api_equivalent_usd: priced ? sum((item) => item.api_equivalent_usd ?? 0) : null,
    unpriced_tokens: sum((item) => item.unpriced_tokens),
    total_tokens: sum((item) => item.total_tokens), wall_seconds: sum((item) => item.wall_seconds),
    tokens: {
      input: sum((item) => item.tokens.input), output: sum((item) => item.tokens.output),
      cache_read: sum((item) => item.tokens.cache_read), cache_write: sum((item) => item.tokens.cache_write),
    },
  };
}
