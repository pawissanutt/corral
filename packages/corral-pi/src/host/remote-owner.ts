import type { ImageContent } from "@earendil-works/pi-ai";
import type { EventLog } from "../bridge/events.ts";
import { BridgeRefusal, type AnswerInput, type RewindInput, type SessionInput, type SubmitResult } from "../bridge/protocol.ts";
import { loadOwnerAttachments } from "./owner-attachments.ts";
import type { HeldInput, SessionRecord } from "./session-record.ts";
import type { Withdrawal } from "./queue.ts";
import { rejectInput } from "./remote/input.ts";
import type { HostSession } from "./session.ts";

/** One ordering point for pause and input admission; provider turns run outside it. */
export class RemoteOwner {
  private transitions: Promise<unknown> = Promise.resolve();
  private flushing: HeldInput | undefined;

  constructor(private readonly host: HostSession, private readonly events: EventLog, private readonly record: SessionRecord,
    private readonly sessionFile: string, private readonly runtimeDir: string) {}

  private order<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.transitions.then(operation);
    this.transitions = result.catch(() => undefined);
    return result;
  }

  get paused(): boolean { return this.record.paused; }

  /**
   * Start automated input now or not at all. It is ordered against pause like every admission, but a paused
   * session refuses it instead of parking it, and the order is held until its turn is open, so a pause that
   * follows cancels that turn rather than finding nothing to cancel.
   */
  start(input: SessionInput, planKickoff?: string): Promise<SubmitResult> {
    return this.order(async () => this.paused ? { disposition: "rejected", reason: "paused" }
      : this.host.submitOpened(input, planKickoff));
  }

  submit(input: SessionInput, images?: ImageContent[], requestId?: string, pane = false): Promise<SubmitResult> {
    const inputId = input.inputId ?? requestId;
    return this.order(async () => {
      if (inputId !== undefined && (this.host.queue.has(inputId) || this.record.held.some(item => item.queueKey === inputId))) {
        return { disposition: "rejected", reason: "input_already_queued" };
      }
      if (inputId !== undefined) this.host.liveEvents.input({ input: inputId, state: "accepted" });
      try {
        const result = await this.admit(input, images, requestId, pane);
        if (inputId !== undefined && result.disposition === "rejected") {
          this.host.liveEvents.input(rejectInput(inputId, result.reason));
        }
        return result;
      } catch (error) {
        if (inputId !== undefined) this.host.liveEvents.input(rejectInput(inputId,
          error instanceof BridgeRefusal ? error.reason : error instanceof Error ? error.message : undefined));
        throw error;
      }
    });
  }

  private async admit(input: SessionInput, images?: ImageContent[], requestId?: string, pane = false): Promise<SubmitResult> {
    if (input.to !== undefined && input.to !== "parent") {
      if (input.attachments?.length || images?.length) return { disposition: "rejected", reason: "child_attachments_unsupported" };
      return await this.host.deliverRemoteChildInput(input.to, input.text)
        ? { disposition: "handled" } : { disposition: "rejected", reason: "child_not_steerable" };
    }
    const loaded = loadOwnerAttachments(this.runtimeDir, input.attachments, this.host.session.model);
    if (input.origin === "auto" && this.paused) {
      const key = input.inputId ?? requestId;
      const queueKey = key ?? `q-${crypto.randomUUID()}`;
      const position = this.host.queue.count(input.busyPolicy)
        + this.record.held.filter(item => item.busyPolicy === input.busyPolicy).length + 1;
      this.record.save(this.sessionFile, true, [...this.record.held, { ...input, queueKey, requestId }]);
      if (key !== undefined) this.host.liveEvents.input({ input: key, state: "queued", policy: input.busyPolicy, position });
      else this.host.liveEvents.stateChanged();
      return { disposition: "queued" };
    }
    const resumed = input.origin === "owner" && await this.flush();
    if (input.origin === "owner" && !input.attachments?.length && !images?.length &&
        await this.host.replyToQuestion(input.text)) return { disposition: "handled" };
    return this.host.submitLocal(
      { ...input, text: input.text + loaded.text, busyPolicy: resumed && input.inputId === undefined ? "steer" : input.busyPolicy },
      images ?? (loaded.images.length ? loaded.images : undefined),
      { requestId, text: input.text, pane },
    );
  }

  queueState(limit: number): Record<string, unknown> {
    return this.host.queue.snapshot(limit, this.record.held);
  }

  queueBrief() {
    return this.host.queue.brief(this.record.held);
  }

  withdraw(key: string): Withdrawal {
    const index = this.record.held.findIndex(item => item.queueKey === key && item !== this.flushing);
    if (index < 0) return this.host.queue.withdrawInput(key, true);
    const held = this.record.held.slice();
    const [item] = held.splice(index, 1);
    this.record.save(this.sessionFile, this.paused, held);
    if (item!.inputId !== undefined || item!.requestId !== undefined) this.host.liveEvents.input({ input: key, state: "withdrawn" });
    else this.host.liveEvents.stateChanged();
    return { outcome: "withdrawn", item: key, text: item!.text };
  }

  /** Pi's idle pane submit has no busy policy: reinject after held input with steer semantics. */
  async paneInput(text: string, images?: ImageContent[]): Promise<boolean> {
    if (!this.paused && !this.record.held.length) return false;
    const result = await this.submit({ text, origin: "owner", busyPolicy: "steer" }, images, undefined, true);
    if (result.disposition === "rejected") throw new Error(result.reason ?? "owner input rejected");
    return true;
  }

  answer(input: AnswerInput): Promise<Record<string, unknown>> {
    return this.order(async () => ({ status: await this.host.answerQuestion(input, () => this.flush()) }));
  }

  pause(): Promise<Record<string, unknown>> {
    return this.order(async () => {
      if (!this.paused) {
        this.record.save(this.sessionFile, true);
        this.host.liveEvents.stateChanged();
        this.events.emit("paused");
      }
      return { ...(await this.host.cancel(undefined)), paused: true };
    });
  }

  rewind(input: RewindInput): Promise<Record<string, unknown>> {
    return this.order(() => this.host.rewind(input));
  }

  resume(): Promise<Record<string, unknown>> {
    return this.order(async () => {
      await this.flush();
      return { paused: false };
    });
  }

  private async flush(): Promise<boolean> {
    if (!this.paused && !this.record.held.length) return false;
    // Retain undelivered input until Pi admits it; a rejection cannot silently drop the tail.
    while (this.record.held.length) {
      const input = this.flushing = this.record.held[0]!;
      try {
        const loaded = loadOwnerAttachments(this.runtimeDir, input.attachments, this.host.session.model);
        const result = await this.host.submitLocal({ ...input, text: input.text + loaded.text,
          busyPolicy: input.inputId === undefined ? "steer" : input.busyPolicy },
          loaded.images.length ? loaded.images : undefined, { requestId: input.requestId, queueKey: input.queueKey, text: input.text });
        if (result.disposition === "rejected") {
          throw new BridgeRefusal("held_input_rejected", `${result.reason ?? "input rejected"}; fix the error and run corral reply or resume the session`);
        }
        this.record.save(this.sessionFile, true, this.record.held.slice(1));
        this.host.liveEvents.stateChanged();
      } finally { this.flushing = undefined; }
    }
    this.record.save(this.sessionFile, false, []);
    this.host.liveEvents.stateChanged();
    this.events.emit("resumed");
    return true;
  }

  async setModel(args: Record<string, unknown>): Promise<Record<string, unknown>> {
    const { model, effort } = args;
    if (typeof model !== "string" || !model.trim() || model.length > 512 || (effort !== undefined && typeof effort !== "string")) {
      throw new BridgeRefusal("bad_input", "model and effort must be strings");
    }
    await this.host.selectRemoteModel(model, effort);
    const selected = this.host.session.model;
    const selection = { model: selected ? `${selected.provider}/${selected.id}` : null, effort: this.host.session.thinkingLevel };
    this.events.emit("owner_model", selection);
    return selection;
  }
}
