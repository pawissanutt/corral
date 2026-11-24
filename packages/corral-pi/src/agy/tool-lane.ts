// A dormant one-prompt lane. Pi executes and records ordinary model-issued tool calls;
// the MCP server only joins an ACP proposal to its HTTP request and returns Pi's result.
import type { Agent } from "@earendil-works/pi-agent-core";
import {
  declarationsEqual, getCurrentSystemPrompt, getCurrentTools, type JsonObject,
  type Model, type SimpleStreamOptions, type Tool, type ToolCall, type TranscriptContext,
} from "@earendil-works/pi-ai";
import { randomUUID } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
import { record, type AgyLaunch } from "./acp.ts";
import type { McpResult } from "./mcp.ts";
import { AgyOutput, driveAgy, type AgyTurn, type ToolExchange } from "./stream.ts";
import { flatObjectSchema, type JsonSchema } from "../tools/schema.ts";

const CONVERSATION_ID = "antigravity.google/conversation_id";
const MAX_CALLS = 128;
const denied = (text: string): McpResult => ({ content: [{ type: "text", text }], isError: true });
const stale = () => denied("Corral tool session is no longer active.");

type RunOwner = Pick<Agent, "signal" | "abort" | "subscribe" | "state">;
type Pending = { call: ToolCall; reply: (result: McpResult) => void };

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

  constructor(launch: AgyLaunch, private readonly model: Model<string>, private readonly turn: AgyTurn,
    readonly tools: readonly Tool[], private readonly owner: RunOwner, readonly runSignal: AbortSignal,
    options: SimpleStreamOptions) {
    this.output = new AgyOutput(model);
    this.turnSignal = AbortSignal.any([runSignal, this.controller.signal]);
    this.finished = driveAgy(launch, model, turn, this.output, { ...options, signal: this.turnSignal }, this);
  }

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

  call: ToolExchange["call"] = async (rpcId, name, args, meta, signal) => {
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
      this.pending = { call: expected, reply: resolve };
      signal.addEventListener("abort", this.revoke, { once: true });
      this.output.handoff(call);
    });
    signal.removeEventListener("abort", this.revoke);
    return reply;
  };

  resume(model: Model<string>, context: TranscriptContext, tools: readonly Tool[], signal: AbortSignal) {
    const pending = this.pending;
    const result = context.messages.at(-1);
    const previous = context.messages.at(-2);
    if (!this.live() || signal !== this.runSignal || !pending || model.id !== this.model.id ||
      model.api !== this.model.api || model.provider !== this.model.provider || !isDeepStrictEqual(tools, this.tools) ||
      getCurrentSystemPrompt(context.messages) !== this.turn.instructions || previous?.role !== "assistant" ||
      !previous.content.some(part => part.type === "toolCall" && isDeepStrictEqual(part, pending.call)) ||
      result?.role !== "toolResult" || result.toolCallId !== pending.call.id || result.toolName !== pending.call.name) return;
    this.pending = undefined;
    pending.reply({ content: result.content, isError: result.isError });
    return this.output.current.stream;
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

/** Bind to one Pi agent. Close on session replacement/shutdown; Pi settlement also revokes the
 * bridge if the loop stops without a continuation. Not registered in any provider catalog. */
export class AgyToolLane {
  private active: Exchange | undefined;
  private closed = false;
  private readonly unsubscribe: () => void;
  private readonly runs = new WeakSet<AbortSignal>();

  constructor(private readonly launch: AgyLaunch, private readonly owner: RunOwner) {
    this.unsubscribe = owner.subscribe(event => {
      if (event.type === "agent_end") this.active?.revoke();
    });
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
    if (this.active) {
      const continuation = this.active.resume(model, context, tools, signal);
      if (continuation) return continuation;
      this.active.revoke();
      return rejected(model, "tool continuation does not match the pending ACP call");
    }
    const user = context.messages.at(-1);
    if (this.runs.has(signal) || user?.role !== "user" ||
      (Array.isArray(user.content) && user.content.some(part => part.type !== "text"))) {
      return rejected(model, "expected one new text prompt, not a replay or stale tool result");
    }
    const prompt = typeof user.content === "string" ? user.content : user.content.map(part => part.type === "text" ? part.text : "").join("\n");
    const turn = { instructions: getCurrentSystemPrompt(context.messages), prompt };
    this.runs.add(signal);
    const active = this.active = new Exchange(this.launch, model, turn, tools, this.owner, signal, options);
    void active.finished.then(() => { if (this.active === active) this.active = undefined; });
    return active.output.current.stream;
  };

  async close(): Promise<void> {
    this.closed = true;
    this.unsubscribe();
    const active = this.active;
    active?.revoke();
    await active?.finished;
  }
}
