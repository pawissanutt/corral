// Shared final error copy for the provider adapter and transcripts. No upstream text is accepted here.
import type { ProviderFailure } from "./provider-failure.ts";

const HEAD = "The OpenAI subscription request failed";
const REASON_UNKNOWN = " The reason could not be determined.";
const STEP = "Send a message to retry, or choose another model.";

const KNOWN_STEPS: Partial<Record<ProviderFailure["category"], string>> = {
  auth: "Sign in again, or choose another model.",
  subscription_quota: "Wait for the usage limit to reset, or choose another model.",
  billing: "Check the account's billing, or choose another model.",
  invalid_request: "Revise the request, shorten the conversation, or choose another model.",
  unavailable: "Choose another model.",
  local_budget: "Check the session's budget, or choose another model.",
  network_policy: "Check the session's network policy before trying again.",
};
const SAFE_STEPS = new Set([STEP, ...Object.values(KNOWN_STEPS)]);

export function codexFailureCopy(input: { partial: boolean; retried?: boolean; requestRef?: string; unknown: boolean;
  category?: ProviderFailure["category"] }): { sentence: string; step: string } {
  const { requestRef } = input;
  const ref = typeof requestRef === "string" && requestRef.length === 64 && /^[0-9a-f]{64}$/i.test(requestRef)
    ? ` (ref ${requestRef.slice(0, 8).toLowerCase()})` : "";
  const retry = input.retried ? " again after one retry" : "";
  const output = input.partial ? "The response stopped early." : "No reply was produced.";
  const reason = input.unknown ? REASON_UNKNOWN : "";
  return { sentence: `${HEAD}${retry}${ref}. ${output}${reason}`, step: input.category ? KNOWN_STEPS[input.category] ?? STEP : STEP };
}

/** Recover the sentence and step from the adapter's canonical text or the legacy normalized summary. */
export function codexFailureCopyFromText(text: string, partial?: boolean): ReturnType<typeof codexFailureCopy> | undefined {
  if (/^(?:Service temporarily unavailable: )?openai-subscription: unknown \((?:account|model|request|unknown)\)$/.test(text)) {
    // A status note with no resident reply has no output facts to preserve.
    return partial === undefined ? { sentence: `${HEAD}.${REASON_UNKNOWN}`, step: STEP } : codexFailureCopy({ partial, unknown: true });
  }
  const sentence = /^(The OpenAI subscription request failed(?: again after one retry)?(?: \(ref [0-9a-f]{8}\))?\. (?:The response stopped early\.|No reply was produced\.)(?: The reason could not be determined\.)?) /.exec(text);
  if (sentence) {
    const step = text.slice(sentence[0].length);
    if (SAFE_STEPS.has(step)) return { sentence: sentence[1]!, step };
  }
  return undefined;
}
