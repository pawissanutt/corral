export type ReadPhase = "not_applicable" | "never_read" | "loading" | "current" | "retained" | "failed";
export interface ReadState {
  readonly phase: ReadPhase;
  readonly readAt: number | undefined;
  readonly failedAt?: number;
  readonly refreshing: boolean;
  readonly error?: string;
  readonly reason?: string;
}
export interface ReadFacts {
  applicable: boolean; reason?: string; hasData: boolean; stale: boolean;
  refreshing: boolean; checkedAt: number | undefined; error: string | undefined; failedAt: number | undefined;
}
export function readState(facts: ReadFacts): ReadState {
  const phase: ReadPhase = !facts.applicable ? "not_applicable"
    : facts.error ? (facts.hasData ? "retained" : "failed")
    : !facts.hasData ? (facts.refreshing ? "loading" : "never_read")
    : facts.stale ? "retained" : "current";
  return { phase, readAt: facts.checkedAt, refreshing: facts.refreshing,
    ...(facts.error === undefined ? {} : { error: facts.error }),
    ...(facts.failedAt === undefined ? {} : { failedAt: facts.failedAt }),
    ...(phase === "not_applicable" && facts.reason ? { reason: facts.reason } : {}) };
}
