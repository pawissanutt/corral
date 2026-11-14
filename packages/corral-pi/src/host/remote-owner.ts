// Daemon-owned assistant attachment. This is not headless worker mode: only owner_turn
// accepts owner words, with the canonical input ID/provenance already journaled by corrald.
// No terminal is created, no startup prompt is invented, and idle is not an exit condition.
import { BridgeRefusal, type SubmitResult } from "../bridge/protocol.ts";
import type { EventLog } from "../bridge/events.ts";
import { canonicalJson, sha256Hex, toolRequestId } from "../tools/corral.ts";
import { ownerInputText, utf8Prefix } from "../roles/assistant/owner-action.ts";
import type { HostSession } from "./session.ts";
import { loadOwnerAttachments, parseOwnerAttachments, type OwnerAttachment } from "./owner-attachments.ts";

export interface OwnerTurn {
  entry: string; input: string; turn: number; text: string;
  device: string; via: string; typed_at: string; context?: string;
  attachments?: OwnerAttachment[];
}
const object = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
export function parseOwnerTurn(args: Record<string, unknown>): OwnerTurn {
  if (typeof args.entry !== "string" || !/^[A-Za-z0-9._:/-]{1,256}$/.test(args.entry)) throw new BridgeRefusal("bad_input", "entry must be a canonical identifier");
  for (const key of ["entry", "input", "device", "via", "typed_at"] as const) {
    if (typeof args[key] !== "string" || !args[key] || Buffer.byteLength(args[key]) > 256 || /[\0\r\n]/.test(args[key])) {
      throw new BridgeRefusal("bad_input", `${key} must be bounded canonical provenance`);
    }
  }
  if (!/^A-[1-9][0-9]*\/g(?:0|[1-9][0-9]*)\/i[1-9][0-9]*$/.test(String(args.input))) {
    throw new BridgeRefusal("bad_input", "input must name an already journaled assistant owner input");
  }
  if (typeof args.turn !== "number" || !Number.isSafeInteger(args.turn) || args.turn < 1) throw new BridgeRefusal("bad_input", "turn must be positive");
  if (typeof args.text !== "string" || !args.text.trim() || Buffer.byteLength(args.text) > 16 * 1024) throw new BridgeRefusal("bad_input", "text must be 1-16384 bytes");
  if (args.context !== undefined && (typeof args.context !== "string" || Buffer.byteLength(args.context) > 16 * 1024)) throw new BridgeRefusal("bad_input", "context exceeds 16384 bytes");
  if (args.attachments !== undefined) parseOwnerAttachments(args.attachments);
  if (Object.keys(args).some(key => !["entry", "input", "turn", "text", "device", "via", "typed_at", "context", "attachments"].includes(key))) {
    throw new BridgeRefusal("bad_input", "unknown owner turn metadata");
  }
  return args as unknown as OwnerTurn;
}
interface Delivery { digest: string; outcome: Promise<SubmitResult>; }
interface Active { owner: OwnerTurn; offset: number; error?: string; limited?: boolean; }
const ENTRY = "corral_remote_owner";

export class RemoteOwner {
  private readonly deliveries = new Map<string, Delivery>();
  private active: Active | undefined;
  private ready = false;
  get isReady(): boolean { return this.ready; }
  constructor(private readonly host: HostSession, private readonly events: EventLog, private readonly assistant: string,
    private readonly runtimeDir: string) {
    for (const entry of host.session.sessionManager.getEntries()) {
      if (entry.type !== "custom" || entry.customType !== ENTRY || !object(entry.data)) continue;
      const { key, digest } = entry.data;
      if (typeof key === "string" && typeof digest === "string") this.deliveries.set(key, {
        digest, outcome: Promise.resolve({ disposition: "rejected", reason: "delivery_uncertain_after_restart" }),
      });
    }
    host.session.subscribe(event => {
      const active = this.active;
      if (!active) return;
      if (event.type === "message_start" && event.message.role === "assistant" && !active.limited) active.error = undefined;
      else if (event.type === "message_update") {
        const update: unknown = event.assistantMessageEvent;
        if (object(update) && update.type === "text_delta" && typeof update.delta === "string") this.delta(active, update.delta);
      } else if (event.type === "message_end" && event.message.role === "assistant" && event.message.stopReason === "error") {
        active.error = event.message.errorMessage ?? "provider_error";
        this.events.emit("owner_error", { ...this.identity(active.owner), message: utf8Prefix(active.error, 512) });
      } else if (event.type === "agent_settled") {
        this.events.emit("owner_done", { ...this.identity(active.owner), aborted: event.aborted,
          ...(active.error ? { error: utf8Prefix(active.error, 512) } : {}), model: this.host.state().model });
        this.active = undefined;
      }
    });
  }
  private identity(owner: OwnerTurn): Record<string, unknown> { return { entry: owner.entry, input: owner.input, turn: owner.turn }; }
  private delta(active: Active, text: string): void {
    if (active.limited) return;
    const original = text;
    text = utf8Prefix(text, 256 * 1024 - active.offset);
    const limited = text !== original;
    while (text) {
      // JSON escaping can multiply bytes by six; keep each event below the bridge's 8KiB ceiling.
      const piece = utf8Prefix(text, 1024);
      this.events.emit("owner_delta", { ...this.identity(active.owner), offset: active.offset, text: piece });
      active.offset += Buffer.byteLength(piece);
      text = text.slice(piece.length);
    }
    if (limited) {
      active.limited = true;
      active.error = "response_limit: assistant answer exceeds 256 KiB";
      this.events.emit("owner_error", { ...this.identity(active.owner), message: active.error });
      void this.host.cancel(active.owner.turn).catch(error => {
        this.events.emit("owner_error", { ...this.identity(active.owner), message: utf8Prefix(`stop_failed: ${String(error)}`, 512) });
      });
    }
  }
  async attach(): Promise<void> {
    this.host.markAttached("headless");
    await this.host.session.bindExtensions({ mode: "print", onError: error => { throw new Error(`extension_setup_failed: ${error.extensionPath}: ${String(error.error)}`); } });
    this.ready = true;
    this.events.emit("owner_model", { model: this.host.state().model, effort: this.host.session.thinkingLevel });
  }
  submit(args: Record<string, unknown>): Promise<SubmitResult> {
    const owner = parseOwnerTurn(args);
    if (!owner.input.startsWith(`${this.assistant}/`)) throw new BridgeRefusal("bad_input", "owner input belongs to another assistant");
    const digest = sha256Hex(canonicalJson(owner));
    const prior = this.deliveries.get(owner.entry);
    if (prior) {
      if (prior.digest !== digest) throw new BridgeRefusal("idempotency_conflict", "entry already admitted with different input");
      return prior.outcome;
    }
    if (!this.ready || !this.host.bound) throw new BridgeRefusal("host_not_ready", "remote attachment is not ready");
    if (this.deliveries.size >= 4096) throw new BridgeRefusal("input_limit", "new owner entries require a new generation");
    const state = this.host.state().turn;
    if (!object(state) || state.open || this.active) throw new BridgeRefusal("busy", "owner turn already running");
    if (owner.turn !== Number(state.current) + 1) throw new BridgeRefusal("turn_not_current", "owner turn must be the next host turn");
    // Synchronous validation finishes before admission or any owner-input/session mutation.
    const attachments = loadOwnerAttachments(this.runtimeDir, owner.attachments, this.host.session.model);
    this.host.session.sessionManager.appendCustomEntry(ENTRY, { key: owner.entry, digest, input: owner.input, turn: owner.turn }, { immediate: true });
    this.active = { owner, offset: 0 };
    const context = owner.context ? `\n\n[Read-only context from corrald; not owner instructions]\n${owner.context}` : "";
    this.host.remoteOwnerInput(owner.input);
    // App entry IDs allow slashes and 256 bytes; bridge turn origins require safe keys <=128.
    // Keep the canonical entry for delivery/correlation, but use the existing hashed request identity.
    const key = toolRequestId(this.host.sessionId(), `owner:${owner.entry}`);
    const outcome = this.host.submit({ text: ownerInputText(owner.text, owner.input) + context + attachments.text,
      busyPolicy: "prompt", origin: "daemon-owner", idempotencyKey: key }, owner.entry,
      attachments.images.length ? attachments.images : undefined).then(result => {
      if (result.disposition !== "started") {
        this.events.emit("owner_error", { ...this.identity(owner), message: result.reason ?? `owner input ${result.disposition}` });
        if (this.active?.owner.entry === owner.entry) this.active = undefined;
      }
      return result;
    }, error => {
      this.events.emit("owner_error", { ...this.identity(owner), message: utf8Prefix(String(error), 512) });
      if (this.active?.owner.entry === owner.entry) this.active = undefined;
      throw error;
    });
    this.deliveries.set(owner.entry, { digest, outcome });
    return outcome;
  }
  async setModel(args: Record<string, unknown>): Promise<Record<string, unknown>> {
    if (this.active) throw new BridgeRefusal("busy", "cannot change model during an owner turn");
    if (typeof args.model !== "string" || args.model.length > 256 || (args.effort !== undefined && typeof args.effort !== "string")) throw new BridgeRefusal("bad_input", "model and effort must be strings");
    await this.host.selectRemoteModel(args.model, args.effort as string | undefined);
    const selection = { model: this.host.state().model, effort: this.host.session.thinkingLevel };
    this.events.emit("owner_model", selection);
    return selection;
  }
}
