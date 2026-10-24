import { record } from "../credentials.mjs";

const STATUS = "anthropic-ratelimit-unified-status";
const CLAIM = "anthropic-ratelimit-unified-representative-claim";
const OVERAGE = "anthropic-ratelimit-unified-overage-status";
const SUBSCRIPTION_CLAIM = /^(five_hour|seven_day[a-z_]*)$/;
const BILLING_TEXT = /extra usage|usage credits|billing|third[- ]party|out of credits|credit balance/i;

/**
 * @typedef {{sequence: number, status: number, headers: Record<string, string>,
 * error?: {type: string | null, message: string | null}, completed: boolean,
 * stopReason?: string, outputTokens?: number, toolCall?: boolean}} Observation
 */

/**
 * What one completed response says about who pays. The evidence is Anthropic's own unified
 * rate-limit headers, the same signal Claude Code shows for plan usage: a representative claim
 * naming a subscription window means the request counted against the plan; a claim of overage
 * (or an overage status that is in use) means extra usage; anything else proves nothing.
 * @param {Observation} observation
 * @returns {"subscription" | "extra_usage" | "unknown"}
 */
export function billingOf(observation) {
  const claim = observation.headers[CLAIM];
  const status = observation.headers[STATUS];
  if (!claim || !status) return "unknown";
  if (claim.startsWith("overage") || observation.headers[OVERAGE] === "in_use") return "extra_usage";
  return SUBSCRIPTION_CLAIM.test(claim) && status !== "rejected" ? "subscription" : "unknown";
}

/** @param {Observation} observation */
function failureReason(observation) {
  const text = `${observation.error?.type ?? ""} ${observation.error?.message ?? ""}`;
  if (BILLING_TEXT.test(text)) return "extra_usage_or_billing_required";
  if (observation.status === 401 || observation.status === 403) return "subscription_token_rejected";
  if (observation.status === 429) return "rate_limited";
  return "request_not_completed";
}

/**
 * GO only with positive evidence of subscription coverage on every completed request and a
 * completed tool-call round trip; everything else is NO-GO with the reason that decided it.
 * @param {readonly Observation[]} observations
 */
export function classifyRoute(observations) {
  const completed = observations.filter((entry) => entry.completed && entry.status >= 200 && entry.status < 300);
  const failed = observations.find((entry) => !entry.completed || entry.status < 200 || entry.status >= 300);
  const billing = completed.map(billingOf);
  const toolRoundTrip = completed.some((entry) => entry.toolCall === true);
  /** @type {"subscription" | "extra_usage" | "unknown" | "blocked"} */
  let summary = "blocked";
  if (completed.length > 0) {
    summary = billing.includes("extra_usage") ? "extra_usage" : billing.every((kind) => kind === "subscription") ? "subscription" : "unknown";
  }
  let reason;
  if (summary === "subscription" && toolRoundTrip && !failed) reason = "subscription_coverage_evidenced";
  else if (failed) reason = failureReason(failed);
  else if (summary === "extra_usage") reason = "extra_usage_billing";
  else if (summary === "unknown") reason = "no_subscription_evidence";
  else if (!toolRoundTrip) reason = "tool_round_trip_not_proven";
  else reason = "no_completed_request";
  return { decision: reason === "subscription_coverage_evidenced" ? "GO" : "NO-GO", billing: summary, toolRoundTrip, reason,
    evidence: observations.map((entry) => ({ sequence: entry.sequence, status: entry.status, billing: entry.completed ? billingOf(entry) : null,
      claim: entry.headers[CLAIM] ?? null, unifiedStatus: entry.headers[STATUS] ?? null, overageStatus: entry.headers[OVERAGE] ?? null,
      fiveHourUtilization: entry.headers["anthropic-ratelimit-unified-5h-utilization"] ?? null })) };
}

export { record };
