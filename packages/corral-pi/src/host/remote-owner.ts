import type { ImageContent } from "@earendil-works/pi-ai";
import type { EventLog } from "../bridge/events.ts";
import { BridgeRefusal, type AnswerInput, type RewindInput, type SessionInput, type SubmitResult } from "../bridge/protocol.ts";
import { loadOwnerAttachments } from "./owner-attachments.ts";
import type { SessionRecord } from "./session-record.ts";
import type { HostSession } from "./session.ts";

/** One ordering point for pause and input admission; provider turns run outside it. */
export class RemoteOwner {
  private transitions: Promise<unknown> = Promise.resolve();

  constructor(private readonly host: HostSession, private readonly events: EventLog, private readonly record: SessionRecord,
    private readonly sessionFile: string, private readonly runtimeDir: string) {}

  private order<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.transitions.then(operation);
    this.transitions = result.catch(() => undefined);
    return result;
  }

  get paused(): boolean { return this.record.paused; }

  submit(input: SessionInput, images?: ImageContent[], requestId?: string): Promise<SubmitResult> {
    return this.order(async () => {
      const loaded = loadOwnerAttachments(this.runtimeDir, input.attachments, this.host.session.model);
      if (input.origin === "auto" && this.paused) {
        this.record.save(this.sessionFile, true, [...this.record.held, input]);
        return { disposition: "queued" };
      }
      const resumed = input.origin === "owner" && await this.flush();
      if (input.origin === "owner" && !input.attachments?.length && !images?.length &&
          await this.host.replyToQuestion(input.text)) return { disposition: "handled" };
      return this.host.submitLocal(
        { ...input, text: input.text + loaded.text, busyPolicy: resumed ? "steer" : input.busyPolicy },
        images ?? (loaded.images.length ? loaded.images : undefined),
        input.origin === "owner" ? requestId : undefined,
      );
    });
  }

  /** Pi's idle pane submit has no busy policy: reinject after held input with steer semantics. */
  async paneInput(text: string, images?: ImageContent[]): Promise<boolean> {
    if (!this.paused && !this.record.held.length) return false;
    const result = await this.submit({ text, origin: "owner", busyPolicy: "steer" }, images);
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
    for (const input of this.record.held) {
      const loaded = loadOwnerAttachments(this.runtimeDir, input.attachments, this.host.session.model);
      const result = await this.host.submitLocal({ ...input, text: input.text + loaded.text, busyPolicy: "steer" },
        loaded.images.length ? loaded.images : undefined);
      if (result.disposition === "rejected") {
        throw new BridgeRefusal("held_input_rejected", `${result.reason ?? "input rejected"}; fix the error and run corral reply or resume the session`);
      }
      this.record.save(this.sessionFile, true, this.record.held.slice(1));
    }
    this.record.save(this.sessionFile, false, []);
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
