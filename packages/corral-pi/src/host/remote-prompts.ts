// Remote owner interactions are canonical records, never UI callbacks disguised as owner
// inputs. Only corrald's authenticated answer services decide; this host opens/observes and
// waits. Poll reads are unjournaled committed envelopes so waiting cannot exhaust receipts.
import { randomBytes } from "node:crypto";
import type { EventLog } from "../bridge/events.ts";
import type { CorralOp } from "../roles/types.ts";
import { canonicalJson, type CorralClient, toolRequestId } from "../tools/corral.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";
import type { RemoteAsk } from "./ask-user.ts";

const object = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
function paused(signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  const { promise, resolve, reject } = Promise.withResolvers<void>();
  const abort = () => { clearTimeout(timer); reject(signal.reason ?? new Error("owner_prompt_aborted")); };
  const timer = setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, 500);
  signal.addEventListener("abort", abort, { once: true });
  return promise;
}
export class RemotePrompts {
  private sequence = 0;
  private input: string | undefined;
  setInput(input: string): void { this.input = input; }
  constructor(private readonly client: CorralClient, private readonly events: EventLog,
    private readonly sessionId: () => string, private readonly turn: () => number | undefined,
    private readonly signal: () => AbortSignal) {}
  private async call(op: CorralOp, payload: Record<string, unknown>, id: string): Promise<Record<string, unknown>> {
    const receipt = await this.client.call(op, { requestId: toolRequestId(this.sessionId(), id), payload, turn: this.turn() });
    if (receipt.state !== "committed" || !object(receipt.result)) throw new Error(`${op}_refused: ${canonicalJson(receipt.result)}`);
    return receipt.result;
  }
  private publish(result: Record<string, unknown>, kind: string, descriptor: Record<string, unknown>): void {
    // The full question/prompt is fetched canonically by app clients; keep the bridge bounded.
    this.events.emit("owner_prompt", { key: result.key, revision: result.revision, state: result.state, kind, ...descriptor });
  }
  private async wait(op: "owner_prompt" | "owner_ask", opened: Record<string, unknown>, kind: string,
    descriptor: Record<string, unknown>, signal: AbortSignal): Promise<Record<string, unknown>> {
    let result = opened;
    let shown = "";
    for (;;) {
      signal.throwIfAborted();
      if (typeof result.key !== "string" || typeof result.revision !== "number" || typeof result.state !== "string") throw new Error(`${op}_invalid_result`);
      const revision = `${result.revision}:${result.state}`;
      if (shown !== revision) { this.publish(result, kind, descriptor); shown = revision; }
      if (!["open", "escalated"].includes(result.state)) {
        if (op === "owner_ask" && result.state === "answered" && !object(result.question)) {
          return this.call(op, { action: "get", key: result.key }, `${op}:answer:${++this.sequence}`);
        }
        return result;
      }
      await paused(signal);
      result = await this.call(op, { action: "get", key: result.key }, `${op}:get:${++this.sequence}:${randomBytes(8).toString("hex")}`);
    }
  }
  async permission(payload: Record<string, unknown>, signal: AbortSignal): Promise<{ authorized: boolean; state: string }> {
    const request = `permission:${randomBytes(16).toString("hex")}`;
    const opened = await this.call("owner_prompt", { ...payload, action: "open", kind: "permission", input: this.input }, request);
    const result = await this.wait("owner_prompt", opened, "permission", {
      tool: payload.tool, path: object(payload.target) ? utf8Prefix(String(payload.target.shown ?? ""), 512) : undefined,
      preview: utf8Prefix(String(payload.preview ?? ""), 512),
    }, signal);
    return { state: String(result.state), authorized: result.state === "answered" && result.authorized === true };
  }
  async confirmation(command: string, request: string): Promise<boolean> {
    const signal = this.signal();
    signal.throwIfAborted();
    const opened = await this.call("owner_prompt", { action: "open", kind: "confirmation", command, request }, `confirmation:${request}`);
    const result = await this.wait("owner_prompt", opened, "confirmation", { command: utf8Prefix(command, 512) }, signal);
    return result.state === "answered" && result.authorized === true;
  }
  private async withdraw(key: string, id: string): Promise<Record<string, unknown>> {
    const current = await this.call("owner_ask", { action: "get", key }, `${id}:get`);
    return this.call("owner_ask", { action: "withdraw", key, revision: current.revision }, id);
  }
  readonly ask: RemoteAsk = async (id, value, toolSignal) => {
    if (!object(value)) throw new Error("owner_ask_invalid_arguments");
    const signal = toolSignal ? AbortSignal.any([toolSignal, this.signal()]) : this.signal();
    const response = (text: string, details: Record<string, unknown>) => ({ content: [{ type: "text" as const, text }], details });
    if (typeof value.withdraw === "string") {
      const result = await this.withdraw(value.withdraw, `ask-withdraw:${id}`);
      this.publish(result, "question", {});
      return response(`${result.key}: ${result.state}; no owner answer`, result);
    }
    if (!Array.isArray(value.questions) || value.questions.length < 1 || value.questions.length > 4) throw new Error("owner_ask_invalid_questions");
    const opened: { question: string; record: Record<string, unknown> }[] = [];
    for (let index = 0; index < value.questions.length; index++) {
      signal.throwIfAborted();
      const q: unknown = value.questions[index];
      if (!object(q) || typeof q.question !== "string" || !Array.isArray(q.options)) throw new Error("owner_ask_invalid_question");
      const options = q.options.map((option: unknown) => typeof option === "string" ? option
        : object(option) && typeof option.label === "string" ? option.label : "");
      if (options.some(option => !option)) throw new Error("owner_ask_invalid_options");
      const descriptions = q.options.flatMap((option: unknown) => object(option) && typeof option.label === "string" && typeof option.description === "string"
        ? [`${option.label}: ${option.description}`] : []);
      const text = [q.question, ...(q.multiSelect === true ? ["Choose all applicable options; record the selections in your answer."] : []), ...descriptions].join("\n");
      const record = await this.call("owner_ask", { action: "open", text, options,
        kind: "clarification", blocking: false }, `owner-ask:${String(value.key ?? id)}:${index}`);
      opened.push({ question: q.question, record });
      this.publish(record, "question", { text: utf8Prefix(q.question, 512) });
    }
    if (typeof value.supersedes === "string") await this.withdraw(value.supersedes, `ask-supersede:${id}`);
    // Remote asks deliberately wait for the canonical decision even if a model requested the
    // terminal's nonBlocking mode. No synthetic local answer or extra user instruction is made.
    const answers: Record<string, unknown>[] = [];
    for (const item of opened) {
      const result = await this.wait("owner_ask", item.record, "question", { text: utf8Prefix(item.question, 512) }, signal);
      const question = result.question;
      const answer = object(question) ? question.answer : undefined;
      if (result.state === "answered" && (!object(answer) || typeof answer.text !== "string")) throw new Error("owner_ask_answer_missing");
      answers.push({ key: result.key, question: item.question, state: result.state,
        ...(result.state === "answered" && object(answer) ? { text: answer.text, option: answer.option } : {}) });
    }
    return response(answers.map(answer => `${answer.question}: ${answer.state === "answered" ? String(answer.text ?? answer.option ?? "") : `${answer.state} (no consent)`}`).join("\n"), { answers });
  };
}
