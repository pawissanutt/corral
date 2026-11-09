// Host API for the Changes panel, not a model tool. Annotation request keys (not host
// generations or transport IDs) own retries. Keep the original action/key on unknown.
import { randomBytes } from "node:crypto";
import { z } from "zod";
import type { CorralClient } from "../tools/corral.ts";

export const ANNOTATION_RESPONSE_BYTES = 32 * 1024;
const uint = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const version = z.literal(1);
const text = z.string();
const state = z.enum(["open", "resolved"]);
const range = z.object({ start: uint.min(1), end: uint.min(1) }).strict()
  .refine(r => r.end >= r.start && r.end - r.start < 400);
const container = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("worker"), task: text, run: text }).strict(),
  z.object({ kind: z.literal("reviewer"), task: text, run: text, review: text }).strict(),
  z.object({ kind: z.literal("planner"), task: text, run: text }).strict(),
  z.object({ kind: z.literal("plan_reviewer"), task: text, plan: text, review: text }).strict(),
  z.object({ kind: z.literal("pm") }).strict(),
  z.object({ kind: z.literal("assistant"), session: text }).strict(),
]);
export const AnnotationAuthorSchema = z.object({ version, kind: z.literal("owner"),
  surface: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("cli") }).strict(),
    z.object({ kind: z.literal("pi_panel"), container, session: text, generation: uint.min(1) }).strict(),
  ]),
}).strict();
export const AnnotationAnchorSchema = z.object({ version, task: text, run: text, base: text,
  head: text, tree: text, path: text, side: z.enum(["new", "old"]), blob: text.nullable(),
  range: range.nullable(), excerpt: text, excerpt_truncated: z.boolean(), excerpt_sha256: text,
}).strict();
export const AnnotationCandidateSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("available"), candidate: z.object({ run: text, base: text, head: text, tree: text }).strict() }).strict(),
  z.object({ status: z.literal("unavailable"), reason: text }).strict(),
]);
export const AnnotationAnchorStatusSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("current") }).strict(),
  z.object({ status: z.literal("stale"), changes: z.array(z.object({ field: text, expected: text, actual: text }).strict()) }).strict(),
  z.object({ status: z.literal("superseded") }).strict(),
  z.object({ status: z.literal("unavailable"), reason: text }).strict(),
]);
export const AnnotationEntrySchema = z.object({ version, seq: uint.min(1).max(64),
  kind: z.enum(["opened", "reply", "resolved", "reopened"]), state, author: AnnotationAuthorSchema,
  at: text, text, request: z.object({ key: text, payload_sha256: text }).strict(),
}).strict();
export const AnnotationItemSchema = z.object({ id: text, revision: uint.min(1).max(64), state,
  anchor: AnnotationAnchorSchema.optional(), anchor_status: AnnotationAnchorStatusSchema,
}).strict();
const view = { version, task: text, candidate: AnnotationCandidateSchema,
  counts: z.object({ threads: uint.max(256), entries: uint.max(16384), text_bytes: uint.max(262144), next_thread: uint }).strict(),
  paths: z.array(text).max(64), paths_truncated: z.boolean(), next_cursor: text.optional() };
export const AnnotationsViewSchema = z.discriminatedUnion("kind", [
  z.object({ ...view, kind: z.literal("list"), items: z.array(AnnotationItemSchema).max(64) }).strict(),
  z.object({ ...view, kind: z.literal("thread"), thread: AnnotationItemSchema,
    entries: z.array(AnnotationEntrySchema).max(64), upper_revision: uint.min(1).max(64) }).strict(),
]).refine(v => Buffer.byteLength(JSON.stringify(v)) <= ANNOTATION_RESPONSE_BYTES, "annotation response exceeds 32 KiB");
export type AnnotationsView = z.infer<typeof AnnotationsViewSchema>;
export const AnnotationCodeSchema = z.enum(["not_owner", "role_not_permitted", "out_of_scope",
  "stale_candidate", "candidate_not_current", "candidate_unavailable", "invalid_path", "path_out_of_scope",
  "invalid_range", "binary_range", "file_too_large", "invalid_text", "thread_not_found",
  "thread_revision_conflict", "invalid_transition", "request_key_conflict", "annotation_limit",
  "unsupported_version", "invalid_cursor"]);
export const AnnotationRefusalSchema = z.object({ code: AnnotationCodeSchema, message: text, detail: z.unknown() }).strict();
export const AnnotationResultSchema = z.object({ task: text, thread: text, entry: uint.min(1).max(64),
  revision: uint.min(1).max(64), state, replayed: z.boolean() }).strict();
export type AnnotationResult = z.infer<typeof AnnotationResultSchema>;
const query = z.object({ task: text, thread: text.optional(), query: z.object({ state: state.optional(),
  anchor: z.enum(["current", "stale", "superseded", "unavailable"]).optional(), cursor: text.optional(),
  limit: uint.min(1).max(64).optional() }).strict().optional() }).strict();
export type AnnotationsQuery = z.infer<typeof query>;
const change = { thread: text, expected_revision: uint.min(1).max(64), text: text.optional() };
export const OwnerAnnotationActionSchema = z.discriminatedUnion("action", [
  z.object({ action: z.literal("open"), task: text, expected_run: text, base: text, head: text,
    path: text, side: z.enum(["new", "old"]), range: range.nullable().optional(), text }).strict(),
  z.object({ action: z.literal("reply"), task: text, ...change, text }).strict(),
  z.object({ action: z.literal("resolve"), task: text, ...change }).strict(),
  z.object({ action: z.literal("reopen"), task: text, ...change }).strict(),
]);
export type OwnerAnnotationAction = z.infer<typeof OwnerAnnotationActionSchema>;
export type AnnotationClient = Pick<CorralClient, "call">;
/** Host supplies an ID from its EventLog using eventRequestId(generation, event.seq).
 * It must not expose this client/callback (or host credentials) to tools or Code Mode. */
export interface OwnerAnnotationClient extends AnnotationClient { nextEventRequestId(): string }
type Refused = { state: "refused"; code: string; detail: unknown };
type Unknown = { state: "unknown"; reason: string };
export type AnnotationRead = { state: "available" | "unavailable"; view: AnnotationsView } | Refused | Unknown;
export type AnnotationSubmission = { state: "committed" | "replayed"; result: AnnotationResult } | Refused | Unknown;
const rejection = z.object({ code: text, detail: z.unknown().optional() });
function refused(value: unknown): Refused | Unknown {
  const parsed = rejection.safeParse(value);
  if (!parsed.success) return { state: "unknown", reason: "invalid_refusal" };
  if (parsed.data.code === "annotation_refused") {
    const error = AnnotationRefusalSchema.safeParse(parsed.data.detail);
    return error.success ? { state: "refused", code: error.data.code, detail: error.data.detail }
      : { state: "unknown", reason: "invalid_refusal" };
  }
  return { state: "refused", code: parsed.data.code, detail: parsed.data.detail };
}
function unknown(error: unknown): Unknown {
  // No transport text, credentials, or note bytes are echoed to the panel.
  const code = error && typeof error === "object" && "code" in error ? error.code : undefined;
  return { state: "unknown", reason: typeof code === "string" && /^[a-z_]{1,64}$/u.test(code) ? code : "transport_failure" };
}
export function newAnnotationRequestKey(): string { return `an-${randomBytes(16).toString("hex")}`; }
export async function readAnnotations(client: AnnotationClient, input: AnnotationsQuery): Promise<AnnotationRead> {
  try {
    const receipt = await client.call("read_annotations", { requestId: `rq-${randomBytes(16).toString("hex")}`, payload: query.parse(input) });
    if (receipt.state === "rejected") return refused(receipt.result);
    if (receipt.state !== "committed") return { state: "unknown", reason: receipt.state };
    const parsed = AnnotationsViewSchema.safeParse(receipt.result);
    if (!parsed.success) return { state: "unknown", reason: "invalid_view" };
    return { state: parsed.data.candidate.status, view: parsed.data };
  } catch (error) { return unknown(error); }
}
/** Call only for an explicit owner panel action. Never change the key or expected identity
 * to retry an unknown result, and never automatically retry a refused concurrency conflict. */
export async function submitOwnerAnnotation(client: OwnerAnnotationClient, input: OwnerAnnotationAction,
  requestKey: string): Promise<AnnotationSubmission> {
  try {
    const { task, ...action } = OwnerAnnotationActionSchema.parse(input);
    if (!/^[A-Za-z0-9._:-]{1,128}$/u.test(requestKey)) return { state: "refused", code: "invalid_text", detail: null };
    const receipt = await client.call("event", { requestId: client.nextEventRequestId(),
      payload: { kind: "owner_annotation", task, request_key: requestKey, action } });
    if (receipt.state === "rejected") return refused(receipt.result);
    if (receipt.state !== "committed") return { state: "unknown", reason: receipt.state };
    const parsed = AnnotationResultSchema.safeParse(receipt.result);
    if (!parsed.success || parsed.data.task !== task) return { state: "unknown", reason: "invalid_result" };
    return { state: parsed.data.replayed ? "replayed" : "committed", result: parsed.data };
  } catch (error) { return unknown(error); }
}
