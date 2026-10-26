import { ComparisonError, canonical, record, text } from "./schema.ts";
import type { Event } from "./schema.ts";

export function uniqueEvents(events: readonly Event[]): readonly Event[] {
  const seen = new Set<string>();
  return events.filter((event) => {
    const d = event.detail;
    const request = text(d, "request_id");
    const key = request
      ? canonical([event.task, event.kind, request])
      : event.kind.startsWith("question_")
        ? canonical([event.task, event.kind, d["key"], d["revision"], d["at"]])
        : canonical(event);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  }).sort((a, b) => Date.parse(a.ts) - Date.parse(b.ts));
}

export function gates(events: readonly Event[]) {
  // Mutable attempt table: each start receives at most one terminal result.
  const open = new Map<string, Event>();
  const rows: { attempt: string; tier: string; result: string; seconds: number }[] = [];
  for (const event of events) {
    if (event.kind !== "gate_started" && event.kind !== "gate") continue;
    const d = event.detail;
    const identity = d["identity"] == null ? {} : record(d["identity"]);
    const attempt = text(identity, "attempt");
    const run = text(d, "run");
    const key = attempt ?? (event.kind === "gate"
      ? [...open.entries()].reverse().find(([, value]) => text(value.detail, "run") === run)?.[0]
      : undefined) ?? `legacy:${run}`;
    if (event.kind === "gate_started") {
      if (open.has(key)) throw new ComparisonError("gate_overlap", key);
      open.set(key, event);
      continue;
    }
    const start = open.get(key);
    if (!start) throw new ComparisonError("gate_unmatched", key);
    open.delete(key);
    const result = text(d, "result");
    if (result !== "pass" && result !== "fail") {
      throw new ComparisonError("gate_result", key);
    }
    const failed = text(d, "failed_cmd") ?? "";
    rows.push({
      attempt: key, tier: text(start.detail, "tier") ?? "full",
      result: result === "pass" ? "pass"
        : failed.startsWith("stale_result") ? "stale_result"
        : failed.startsWith("invalid_command_receipts") ? "invalid_receipts" : "command_failed",
      seconds: (Date.parse(event.ts) - Date.parse(start.ts)) / 1000,
    });
  }
  if (open.size) throw new ComparisonError("gate_unfinished", [...open.keys()].join(", "));
  return rows;
}

const rescueKinds = new Set([
  "replied", "message_retried", "inbox_recovered", "unstuck",
  "reply_retried", "reply_marked_delivered",
]);

function paneAudit(content: string): boolean {
  if (!content.trim().startsWith("{")) return false;
  try {
    const audit: unknown = JSON.parse(content);
    const kind = record(audit)["comparison_rescue"];
    return kind === "pane_input" || kind === "prerequisite_context";
  } catch (error) {
    // Ordinary comment text, including a leading brace, is not a rescue audit.
    if (error instanceof SyntaxError) return false;
    throw error;
  }
}

export function interventions(events: readonly Event[], end: number) {
  let needsMeCount = 0;
  let needsMeSeconds = 0;
  let opened: number | undefined;
  let rescues = 0;
  let pmAnswers = 0;
  let ownerAnswers = 0;
  const decisions = new Set<string>();
  for (const event of events) {
    const d = event.detail;
    const at = Date.parse(event.ts);
    if (rescueKinds.has(event.kind) && d["submitted"] !== false && d["control"] !== true) rescues++;
    if (["message_failed", "message_abandoned", "message_superseded", "message_cancelled"].includes(event.kind)
      && d["via"] === "manual") rescues++;
    if (event.kind === "message_observed" && d["receipt"] != null
      && record(d["receipt"])["via"] === "manual") rescues++;
    if (event.kind === "commented" && event.actor === "pm" && d["comment"] != null) {
      const content = text(record(d["comment"]), "text");
      // A machine-consumed JSON comment is an explicit audit, never a prose heuristic.
      if (content && paneAudit(content)) rescues++;
    }
    if (event.kind === "needs_me") {
      needsMeCount++;
      opened ??= at;
      const decision = text(d, "decision");
      if (decision) decisions.add(decision);
    }
    const decisionAnswer = event.kind === "run_resumed" && d["outcome"] === "answered";
    if (decisionAnswer) {
      const decision = text(d, "decision") ?? text(d, "receipt");
      if (decision) decisions.delete(decision);
    }
    const reply = event.kind === "replied" && d["submitted"] !== false && !decisions.size;
    const marked = event.kind === "reply_marked_delivered" && d["control"] === false
      && d["resumes"] === true && !decisions.size;
    const closes = event.kind === "resumed" || event.kind === "done"
      || event.kind === "discarded" || event.kind === "approved"
      || (decisionAnswer && !decisions.size) || reply || marked;
    if (closes && opened !== undefined) {
      needsMeSeconds += (at - opened) / 1000;
      opened = undefined;
    }
    if (event.kind === "question_answered" || event.kind === "question_expired") {
      const by = text(d, "by");
      if (by === "pm") pmAnswers++;
      else if (by === "owner" || by === "owner_deadline" || event.kind === "question_expired") ownerAnswers++;
      else throw new ComparisonError("answer_authority", event.ts);
    }
  }
  if (opened !== undefined) needsMeSeconds += (end - opened) / 1000;
  return { rescues, needs_me: needsMeCount, needs_me_seconds: needsMeSeconds,
    pm_answers: pmAnswers, owner_answers: ownerAnswers };
}
