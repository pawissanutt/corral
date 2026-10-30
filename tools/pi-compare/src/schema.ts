import { z } from "zod";
import { createHash } from "node:crypto";

export class ComparisonError extends Error {
  constructor(readonly code: string, message: string) {
    super(`${code}: ${message}`);
    this.name = "ComparisonError";
  }
}

export const timestamp = z.string().refine(
  (value) => Number.isFinite(Date.parse(value)), "invalid timestamp",
);
const count = z.number().int().nonnegative().safe();
const money = z.number().finite().nonnegative().nullable();
export const digest = z.string().regex(/^[a-f0-9]{64}$/);
const usage = z.object({
  input: count, output: count, cache_read: count, cache_write: count,
  estimated_usd: money, reported_usd: money, unpriced_tokens: count,
});
const turn = z.object({
  generation: count, seq: count, ended: timestamp.nullish(),
  class: z.string().nullish(),
});
const host = z.object({ turns: z.array(turn).default([]) });
const review = z.object({
  id: z.string(), started: timestamp, ended: timestamp.nullish(),
  agent: z.string(), usage: usage.nullish(), agent_host: host.nullish(),
});
export const runSchema = z.object({
  id: z.string(), harness: z.enum(["omo", "pi-native"]),
  model: z.string(), effort: z.string(), base_sha: z.string().min(1),
  started: timestamp, usage: usage.nullish(),
  review: z.object({ reviewer: z.string(), verdict: z.string().nullish() }).nullish(),
  review_runs: z.array(review).default([]), agent_host: host.nullish(),
});
export const showSchema = z.object({
  id: z.string(), body: z.string(), kind: z.string(),
  size: z.enum(["xs", "s", "m", "l"]),
  gate_contract: z.record(z.string(), z.unknown()),
  runs: z.array(runSchema),
});
export type Run = z.infer<typeof runSchema>;
export type Show = z.infer<typeof showSchema>;

export const eventSchema = z.object({
  ts: timestamp, task: z.string(), kind: z.string(),
  actor: z.string().nullish(), via: z.string().nullish(),
  detail: z.record(z.string(), z.unknown()),
});
export type Event = z.infer<typeof eventSchema>;
const snapshot = z.object({
  binary_sha256: digest, config_sha256: digest,
  gate_contract_sha256: digest,
  pm_harness: z.literal("omo"), pm_profile: z.string().min(1),
  reviewer_profile: z.string().min(1), occupied_slots: count,
}).strict();
const arm = z.object({
  start_show: z.string().min(1),
  show: z.string().min(1), events: z.string().min(1), run: z.string().min(1),
  event_surface: z.enum(["log_all", "export_events"]),
  before: snapshot, after: snapshot,
}).strict();
export const pairSchema = z.object({
  id: z.string().min(1), expected_tokens: count.positive(),
  source_task: z.string().min(1),
  selection: z.string().min(1),
  order: z.array(z.enum(["omo", "pi-native"])).length(2),
  omo: arm, pi_native: arm,
}).strict();
export const manifestSchema = z.object({
  version: z.literal(1), seed: z.string().min(1),
  data_kind: z.enum(["synthetic", "measured"]),
  pairs: z.array(pairSchema).min(1).max(6),
}).strict();
export type Pair = z.infer<typeof pairSchema>;
export type Manifest = z.infer<typeof manifestSchema>;
export const selectionSchema = showSchema.pick({ id: true, body: true, kind: true, size: true })
  .extend({ status: z.literal("backlog") });

// Canonical comparison hashing, not a replacement for corral's own gate identity hash.
export function canonical(value: unknown): string {
  if (value === null || typeof value !== "object") {
    const encoded = JSON.stringify(value);
    if (encoded === undefined) throw new ComparisonError("invalid_json", "undefined value");
    return encoded;
  }
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  const entries = Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${canonical(item)}`).join(",")}}`;
}
export function sha256(value: string | Uint8Array): string {
  return createHash("sha256").update(value).digest("hex");
}
export function assignedOrder(seed: string, id: string): readonly string[] {
  const first = Number.parseInt(sha256(`${seed}\n${id}`).slice(0, 2), 16) % 2;
  return first === 0 ? ["omo", "pi-native"] : ["pi-native", "omo"];
}
export function text(detail: Readonly<Record<string, unknown>>, key: string): string | undefined {
  const value = detail[key];
  return typeof value === "string" ? value : undefined;
}
export function record(value: unknown): Readonly<Record<string, unknown>> {
  return z.record(z.string(), z.unknown()).parse(value);
}

export function parseEvents(raw: string): readonly Event[] {
  const trimmed = raw.trim();
  if (!trimmed) return [];
  const decoded: unknown = trimmed.startsWith("[")
    ? JSON.parse(trimmed)
    : trimmed.split("\n").filter((line) => line.trim()).map((line): unknown => JSON.parse(line));
  return z.array(z.unknown()).parse(decoded).flatMap((item) => {
    const row = record(item);
    const event = record(row["event"] ?? row);
    // Whole-store exports also contain unscoped PM/config events, outside this experiment.
    if (event["task"] == null) return [];
    return [eventSchema.parse(event)];
  });
}
