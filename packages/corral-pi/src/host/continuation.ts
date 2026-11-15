// Rust's committed hello receipt is the only continuation authority in the host.
import { createHash } from "node:crypto";
import { closeSync, fstatSync, openSync, readFileSync } from "node:fs";
import { z } from "zod";
import type { ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";
import { CONNECTIONS, isApiConnection, isConnectionId } from "../auth/connections.ts";
import type { ConnectionId } from "../auth/connections.ts";
import type { EventLog } from "../bridge/events.ts";
import { continuationContract } from "../roles/common/continuation.ts";
import type { HostIdentity, RoleContext } from "../roles/types.ts";
import type { ConnectionBinding } from "./connections.ts";
import type { ProviderFailure } from "./provider-failure.ts";

export const CONTINUE_ASSISTANT_EXIT = 78;
const ENTRY = "corral_continuation_notice";
const CONTEXT_MAX = 256 * 1024;
const text = (max: number) => z.string().min(1).refine(value => value.trim() !== ""
  && Buffer.byteLength(value) <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value));
const digest = z.string().regex(/^[a-f0-9]{64}$/);
const integer = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const connection = z.custom<ConnectionId>(isConnectionId);
const Notice = z.object({
  version: z.literal(1), generation: integer.min(1).max(0xffff_ffff), binding_digest: digest,
  source_connection: connection, target_connection: connection, candidate: text(512).nullable(),
  reason: text(4096), episode: text(256), allowance_nanos: integer.min(1), consumed_or_reserved_nanos: integer,
  context: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("checkpoint"), digest, path: text(4096).refine(value => value.startsWith("/")),
      bytes: integer.max(CONTEXT_MAX) }).strict(),
    z.object({ kind: z.literal("fresh-read-only"), reason: text(4096) }).strict(),
    z.object({ kind: z.literal("resumed-session") }).strict(),
  ]),
}).strict();

export type ContinuationNotice = z.infer<typeof Notice>;

export function parseNotice(value: unknown, identity: HostIdentity, binding: ConnectionBinding | undefined): ContinuationNotice | undefined {
  if (value === undefined || value === null) return undefined;
  const parsed = Notice.safeParse(value);
  if (!parsed.success) throw new Error("continuation_invalid: malformed notice");
  const notice = parsed.data;
  if (notice.generation !== identity.generation) throw new Error("continuation_invalid: generation mismatch");
  if (!binding || notice.binding_digest !== binding.digest) throw new Error("continuation_invalid: binding digest mismatch");
  if (notice.target_connection !== binding.connection || notice.candidate !== binding.candidate
      || !isApiConnection(notice.target_connection)
      || CONNECTIONS[notice.source_connection].vendor !== CONNECTIONS[notice.target_connection].vendor) {
    throw new Error("continuation_invalid: connection or candidate mismatch");
  }
  if (notice.context.kind === "fresh-read-only" && !["reviewer", "planner", "plan_reviewer"].includes(identity.role)) {
    throw new Error("continuation_invalid: role requires committed context");
  }
  if (notice.context.kind === "resumed-session" && !["pm", "assistant"].includes(identity.role)) {
    throw new Error("continuation_invalid: role cannot resume a retained session");
  }
  return notice;
}

export function noticeText(notice: ContinuationNotice): string {
  return `corral: ${notice.source_connection} → ${notice.target_connection}: ${notice.reason}; `
    + `candidate ${notice.candidate ?? "none"}, episode ${notice.episode}; `
    + `allowance ${notice.allowance_nanos} nanos, consumed/reserved ${notice.consumed_or_reserved_nanos} nanos`;
}

export function readContext(notice: ContinuationNotice): string | undefined {
  if (notice.context.kind !== "checkpoint") return undefined;
  const context = notice.context;
  let bytes: Buffer;
  try {
    const fd = openSync(context.path, "r");
    try {
      const stat = fstatSync(fd);
      if (!stat.isFile() || stat.size !== context.bytes || stat.size > CONTEXT_MAX) {
        throw new Error("size mismatch");
      }
      bytes = readFileSync(fd);
    } finally { closeSync(fd); }
  } catch { throw new Error("continuation_context_invalid: context artifact unavailable or size mismatch"); }
  if (bytes.length !== context.bytes || createHash("sha256").update(bytes).digest("hex") !== context.digest) {
    throw new Error("continuation_context_invalid: context artifact size or SHA-256 mismatch");
  }
  return bytes.toString("utf8");
}

/** Host plumbing only: no route selection, financial admission or provider retry owner. */
export class HostContinuation {
  notice: ContinuationNotice | undefined;
  private context: string | undefined;
  private failure: { turn: number; failure: ProviderFailure } | undefined;

  constructor(private readonly options: {
    identity: HostIdentity; binding: () => ConnectionBinding | undefined; manager: SessionManager;
    events: EventLog; notify: (text: string) => void; restart?: () => void;
  }) {}

  accept(value: unknown): void {
    const o = this.options;
    const binding = o.binding();
    const notice = parseNotice(value, o.identity, binding);
    if (!notice && binding && isApiConnection(binding.connection)) {
      throw new Error("paid_admission_required: API hello has no continuation notice");
    }
    // Verify before publishing the bound state, notice marker or model context.
    const context = notice ? readContext(notice) : undefined;
    this.notice = notice;
    this.context = context;
  }

  /** Wait for the host's presentation sink, so fullscreen startup cannot erase the notice. */
  show(): void {
    const o = this.options;
    const notice = this.notice;
    if (!notice) return;
    const key = createHash("sha256").update(JSON.stringify([o.identity, notice.binding_digest])).digest("hex");
    if (o.manager.getEntries().some(entry => entry.type === "custom" && entry.customType === ENTRY
        && typeof entry.data === "object" && entry.data !== null && "key" in entry.data && entry.data.key === key)) return;
    o.manager.appendCustomEntry(ENTRY, { key, generation: notice.generation }, { immediate: true });
    o.notify(noticeText(notice));
    o.events.emit("continuation_notice", { ...notice });
  }

  extend(pi: ExtensionAPI, context: RoleContext): void {
    // Prepending at the finalized context boundary makes this the first message, not owner
    // input or an automatically replayed turn. It also survives compaction and host reloads.
    pi.on("context_with_system", event => {
      const contract = continuationContract(context);
      if (!contract) return undefined;
      const manifest = ["pm", "assistant"].includes(context.identity.role) ? this.context : undefined;
      return { messages: [{ role: "user" as const, timestamp: 0,
        content: contract + (manifest === undefined ? "" : `\n\n[corral verified continuation context]\n${manifest}`) }, ...event.messages] };
    });
  }

  started(): void { this.failure = undefined; }

  failed(turn: number, failure: ProviderFailure): void { this.failure = { turn, failure }; }

  quota(turn: number): boolean {
    const failure = this.failure;
    return this.options.identity.role === "assistant" && this.options.restart !== undefined
      && failure?.turn === turn && failure.failure.category === "subscription_quota"
      && CONNECTIONS[failure.failure.connection].billing === "subscription";
  }

  request(turn: number, idle: () => boolean): boolean {
    if (!this.quota(turn) || !idle()) return false;
    this.failure = undefined;
    this.options.restart?.();
    return true;
  }
}
