// A dormant lane bound to one Pi conversation. Pi executes and records model-issued tool calls;
// the MCP server only joins an ACP proposal to its HTTP request and returns Pi's result.
import type { Agent, AgentMessage } from "@earendil-works/pi-agent-core";
import {
  declarationsEqual, getCurrentSystemPrompt, getCurrentTools, type JsonObject,
  type Model, type SimpleStreamOptions, type Tool, type ToolCall, type ToolResultMessage, type TranscriptContext,
} from "@earendil-works/pi-ai";
import { randomUUID } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
import { AcpFailure, bounded, record, type AgyLaunch } from "./acp.ts";
import { renderPrompt, transcriptKeys } from "./history.ts";
import type { McpResult } from "./mcp.ts";
import { AgyOutput, driveAgy, type AgySession, type AgyTurn, type SessionHandoff, type ToolExchange } from "./stream.ts";
import { flatObjectSchema, type JsonSchema } from "../tools/schema.ts";

const CONVERSATION_ID = "antigravity.google/conversation_id";
const MAX_CALLS = 128;
const TERMINAL_RESULT_DRAIN_MS = 1_000;
const IDLE_SESSION_MS = 60_000;
const denied = (text: string): McpResult => ({ content: [{ type: "text", text }], isError: true });
const stale = () => denied("Corral tool session is no longer active.");

type RunOwner = Pick<Agent, "signal" | "abort" | "subscribe" | "state">;
type Scope = Pick<Model<string>, "id" | "api" | "provider"> & Pick<SimpleStreamOptions, "sessionId" | "reasoning" | "toolChoice"> & {
  instructions: string;
  tools: readonly Tool[];
};
type Pending = {
  call: ToolCall;
  reply: (result: McpResult) => void;
  responseDrained: Promise<void>;
  assistantKey?: string;
  recorded?: ToolResultMessage;
};
type Resident = { session: AgySession; scope: Scope; history: readonly string[]; timer: ReturnType<typeof setTimeout> };

class Exchange implements ToolExchange {
  private readonly prefix = `agy_${randomUUID()}_`;
  private readonly controller = new AbortController();
  private readonly grants = new Map<string, ToolCall>();
  private readonly proposed = new Set<string>();
  private readonly requests = new Set<string | number>();
  private readonly dispatched: ToolCall[] = [];
  private scope: { sessionId: string; mcpPrefix: string } | undefined;
  private stopped = false;
  private pending: Pending | undefined;
  readonly output: AgyOutput;
  readonly finished: Promise<void>;
  private readonly turnSignal: AbortSignal;

  constructor(launch: AgyLaunch, model: Model<string>, turn: AgyTurn, private readonly binding: Scope,
    private history: readonly string[], private readonly owner: RunOwner, readonly runSignal: AbortSignal,
    options: SimpleStreamOptions, handoff: SessionHandoff) {
    this.output = new AgyOutput(model);
    this.turnSignal = AbortSignal.any([runSignal, this.controller.signal]);
    this.finished = driveAgy(launch, model, turn, this.output, { ...options, signal: this.turnSignal }, this, handoff);
  }

  get tools(): readonly Tool[] { return this.binding.tools; }

  private live(): boolean {
    return !this.stopped && !this.turnSignal.aborted && this.owner.signal === this.runSignal;
  }

  activate(sessionId: string, serverName: string): void {
    this.scope = { sessionId, mcpPrefix: `mcp_${serverName}_` };
  }

  // Agy needs an allow_once to reach *our* HTTP bridge. This is not approval of the
  // effect: the bridge never executes, and Pi rules/owner approval still decide it.
  permit: ToolExchange["permit"] = params => {
    const scope = this.scope;
    if (!scope || !this.live() || params.sessionId !== scope.sessionId || this.proposed.size >= MAX_CALLS) return;
    const proposal = record(params.toolCall);
    const id = proposal?.toolCallId;
    const name = proposal?.name;
    if (typeof id !== "string" || !/^[-\w.:]{1,128}$/.test(id) || this.proposed.has(id) ||
      typeof name !== "string" || !name.startsWith(scope.mcpPrefix)) return;
    const tool = this.tools.find(tool => tool.name === name.slice(scope.mcpPrefix.length));
    const input = record(proposal?.rawInput);
    if (!tool || !input || !this.owner.state.tools.some(active => declarationsEqual(active, tool))) return;
    const options = Array.isArray(params.options) ? params.options.map(record) : [];
    const allow = options.find(option => option?.kind === "allow_once")?.optionId;
    if (typeof allow !== "string" || !allow || options.filter(option => option?.optionId === allow).length !== 1) return;
    // Agy adds toolSummary to its ACP proposal and strips it from the MCP arguments.
    // Never discard a field that belongs to Pi's schema.
    const args = { ...input } as JsonObject; // ACP frames have already been parsed from JSON.
    const schema = flatObjectSchema(tool.parameters as JsonSchema);
    if (typeof args.toolSummary === "string" && !Object.hasOwn(schema.properties ?? {}, "toolSummary")) delete args.toolSummary;
    this.proposed.add(id);
    this.grants.set(id, { type: "toolCall", id: this.prefix + id, name: tool.name, arguments: args });
    return allow;
  };

  call: ToolExchange["call"] = async (rpcId, name, args, meta, signal, responseDrained) => {
    if (!this.scope || !this.live() || signal.aborted || meta?.[CONVERSATION_ID] !== this.scope.sessionId) return stale();
    if (this.requests.has(rpcId) || this.requests.size >= MAX_CALLS) return denied("Duplicate or excess Corral tool request.");
    this.requests.add(rpcId);
    const matches = [...this.grants].filter(([, call]) => call.name === name && isDeepStrictEqual(call.arguments, args));
    // ACP and MCP use distinct IDs. Never guess if parallel identical proposals are ambiguous.
    if (matches.length !== 1) {
      for (const [id] of matches) this.grants.delete(id);
      return denied("No unique one-time ACP grant matches this Corral tool request.");
    }
    const [id, call] = matches[0]!;
    this.grants.delete(id);
    if (this.pending) return denied("Another Corral tool is pending; retry after its result.");
    const tool = this.tools.find(tool => tool.name === call.name);
    if (!tool || !this.owner.state.tools.some(active => declarationsEqual(active, tool))) return stale();
    // Agy does not carry the ACP call ID in MCP. Even a fresh MCP ID could be a retry
    // that stole a later identical grant, so this prompt can hand off each tuple only once.
    if (this.dispatched.some(prior => prior.name === name && isDeepStrictEqual(prior.arguments, args))) {
      return denied("An identical Corral tool request already ran or was denied in this prompt.");
    }
    const expected = structuredClone(call);
    this.dispatched.push(expected);
    const reply = await new Promise<McpResult>(resolve => {
      const pending: Pending = this.pending = { call: expected, reply: resolve, responseDrained };
      signal.addEventListener("abort", this.revoke, { once: true });
      pending.assistantKey = transcriptKeys([this.output.handoff(call)])[0];
    });
    signal.removeEventListener("abort", this.revoke);
    return reply;
  };

  recordResult(message: AgentMessage, signal: AbortSignal): void {
    const pending = this.pending;
    if (signal === this.runSignal && pending && message.role === "toolResult" &&
      pending.call.id === message.toolCallId && pending.call.name === message.toolName) {
      pending.recorded = structuredClone(message);
    }
  }

  private complete(): Pending | undefined {
    const pending = this.pending;
    const result = pending?.recorded;
    if (!this.live() || !result) return;
    this.pending = undefined;
    pending.reply({ content: result.content, isError: result.isError });
    return pending;
  }

  resume(binding: Scope, history: readonly string[], signal: AbortSignal) {
    const pending = this.pending;
    if (signal !== this.runSignal || !pending?.recorded || !isDeepStrictEqual(binding, this.binding) ||
      history.at(-2) !== pending.assistantKey || history.at(-1) !== transcriptKeys([pending.recorded])[0] ||
      !isDeepStrictEqual(history.slice(0, -2), this.history) || !this.complete()) return;
    this.history = history;
    return this.output.current.stream;
  }

  /** Pi completed the effect, but edited/compacted the context before its next model call.
   * Do not send a result to the old model and do not abort Pi's new request. */
  discardAfterResult(signal: AbortSignal): boolean {
    const pending = this.pending;
    if (signal !== this.runSignal || !this.live() || !pending?.recorded) return false;
    this.pending = undefined;
    pending.reply(stale());
    this.revoke();
    return true;
  }

  async settle(signal: AbortSignal): Promise<void> {
    if (signal !== this.runSignal) return;
    // Pi may terminate after recording the tool result, without requesting another model
    // segment. Return that same result, but close admission before yielding to the HTTP writer.
    const answered = this.complete();
    this.stop();
    if (answered) {
      try {
        await bounded(() => answered.responseDrained, TERMINAL_RESULT_DRAIN_MS, "terminal tool result", this.turnSignal);
      } catch { /* A cancelled or unresponsive peer must not hold the Pi run open. */ }
    }
    this.revoke();
    await this.finished;
  }

  stop(): void {
    this.stopped = true;
    this.scope = undefined;
    this.grants.clear();
    const pending = this.pending;
    this.pending = undefined;
    // Revoke in Pi's own run, including while its approval hook is awaiting the owner.
    // Compare the signal so a late event can never abort somebody else's run.
    if (pending && this.owner.signal === this.runSignal) this.owner.abort();
    pending?.reply(stale());
  }

  revoke = (): void => {
    this.stop();
    this.controller.abort();
  };
}

function rejected(model: Model<string>, why: string) {
  const { message, stream } = new AgyOutput(model).current;
  message.stopReason = "error";
  message.errorMessage = `agy ACP: ${why}`;
  stream.push({ type: "error", reason: "error", error: message });
  stream.end();
  return stream;
}

/** Bind to one Pi agent, never shared between a parent and child. Close on session replacement
 * or shutdown. The caller still owns workspace/process-state isolation (see AgyLaunch).
 * Not registered in any provider catalog. */
export class AgyToolLane {
  private active: Exchange | undefined;
  private resident: Resident | undefined;
  private disposal: Promise<void> | undefined;
  private closed = false;
  private readonly unsubscribe: () => void;
  private readonly requests = new WeakMap<AbortSignal, readonly string[]>();

  constructor(private readonly launch: AgyLaunch, private readonly owner: RunOwner) {
    this.unsubscribe = owner.subscribe((event, signal) => {
      if (event.type === "message_end") this.active?.recordResult(event.message, signal);
      if (event.type === "agent_end") {
        if (signal.aborted) this.retireResident();
        return this.active?.settle(signal);
      }
    });
  }

  private retireResident(): void {
    const resident = this.resident;
    if (!resident) return;
    this.resident = undefined;
    clearTimeout(resident.timer);
    this.disposal = resident.session.peer.close();
    // Keep the rejection for the next acquisition/close, without an unhandled idle rejection.
    void this.disposal.catch(() => undefined);
  }

  private takeResident(scope: Scope, history: readonly string[], context: TranscriptContext): AgySession | undefined {
    const resident = this.resident;
    if (!resident) return;
    if (resident.session.peer.live && isDeepStrictEqual(scope, resident.scope) &&
      context.messages.at(-1)?.role === "user" && isDeepStrictEqual(history.slice(0, -1), resident.history)) {
      this.resident = undefined;
      clearTimeout(resident.timer);
      return resident.session;
    }
    this.retireResident();
  }

  stream = (model: Model<string>, context: TranscriptContext, options?: SimpleStreamOptions) => {
    const signal = options?.signal;
    if (this.closed || !options || !signal || signal !== this.owner.signal || signal.aborted) {
      return rejected(model, "tool lane requires its bound Pi run");
    }
    // Extensions can transform a provider transcript. It may narrow but must not enlarge
    // the executable loadout installed on this agent by the role.
    const activeTools = this.owner.state.tools;
    const tools = options.toolChoice === "none" ? [] : getCurrentTools(context.messages)
      .filter(tool => activeTools.some(active => declarationsEqual(active, tool)));
    const { id, api, provider } = model;
    const { sessionId, reasoning, toolChoice } = options;
    const scope: Scope = structuredClone({ id, api, provider, sessionId, reasoning, toolChoice, tools,
      instructions: getCurrentSystemPrompt(context.messages) });
    const history = transcriptKeys(context.messages);
    const previous = this.active;
    if (previous) {
      const continuation = previous.resume(scope, history, signal);
      if (continuation) return continuation;
      if (!previous.discardAfterResult(signal)) {
        previous.revoke();
        return rejected(model, "tool continuation does not match the pending ACP call");
      }
    }
    if (!context.messages.some(message => message.role !== "system") || isDeepStrictEqual(this.requests.get(signal), history)) {
      return rejected(model, "expected a new Pi transcript request");
    }
    const resident = this.takeResident(scope, history, context);
    const messages = context.messages.slice();
    const turn = { instructions: scope.instructions, prompt: renderPrompt(messages, resident !== undefined) };
    const seed = () => {
      // A payload hook can yield to an extension holding the same Pi message objects.
      // Recheck before reseeding so the recovered payload and its retained keys agree.
      if (!isDeepStrictEqual(transcriptKeys(messages), history)) {
        throw new AcpFailure("protocol", "Pi transcript changed while preparing the prompt");
      }
      return renderPrompt(messages, false);
    };
    this.requests.set(signal, history);
    const active = this.active = new Exchange(this.launch, model, turn, scope, history, this.owner, signal, options, {
      ready: Promise.all([previous?.finished, this.disposal]).then(() => undefined),
      reuse: resident ? { session: resident, seed } : undefined,
      retain: (session, message) => {
        // ACP permission requests and the MCP conversation ID carry no prompt epoch. A
        // bridged session cannot safely carry authority over to the next prompt, even if
        // tools are unchanged. Its next turn is seeded from Pi's recorded results instead.
        if (scope.tools.length || this.closed || signal.aborted) return false;
        const timer = setTimeout(() => this.retireResident(), IDLE_SESSION_MS);
        timer.unref();
        const resident = this.resident = { session, scope, history: [...history, ...transcriptKeys([message])], timer };
        session.peer.watchIdle(() => { if (this.resident === resident) this.retireResident(); });
        return true;
      },
    });
    void active.finished.then(() => { if (this.active === active) this.active = undefined; });
    return active.output.current.stream;
  };

  async close(): Promise<void> {
    this.closed = true;
    this.unsubscribe();
    const active = this.active;
    active?.revoke();
    this.retireResident();
    await Promise.all([active?.finished, this.disposal]);
  }
}
