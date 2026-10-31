// The implement-now workflow as the loaded assistant contract states it (T-653), so tests bind their
// scripted turns to the production contract instead of a copy that could drift from it.

/** The one plain implement-now or queue question the contract tells the assistant to ask. */
export function implementNowQuestion(contract: string): string | undefined {
  return /ask\s+once,\s+in\s+plain\s+words:\s+"([^"]+)"/.exec(contract)?.[1]?.replace(/\s+/g, " ");
}

/** The numbered choices the contract's completion report offers, by label. */
export function completionChoices(contract: string): string[] {
  return [...contract.matchAll(/^\d\. ([A-Z][a-z ]+):/gm)].map((match) => match[1] ?? "");
}
