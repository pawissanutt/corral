// Dormant ACP transport. Do not register this as a selectable provider: T-2007
// found that agy 1.5.14 can bypass host tool mediation. Role instructions are not isolation.
import {
  createAssistantMessageEventStream, type AssistantMessage, type AssistantMessageEventStream,
  type Model, type SimpleStreamOptions, type Tool, type ToolCall,
} from "@earendil-works/pi-ai";
import { AcpFailure, AcpPeer, bounded, deadlineMs, executorWarning, record, rpcFailure, type AcpPermission, type AgyLaunch } from "./acp.ts";
import { isDeepStrictEqual } from "node:util";
import type { AgyPromptBlock } from "./history.ts";
import { openAgyMcp, type AgyMcp } from "./mcp.ts";

export interface AgyTurn {
  /** Trusted Corral role instructions; sent in the negotiated system-prompt extension. */
  instructions: string;
  prompt: string | readonly AgyPromptBlock[];
}

// ACP has no HTTP status/headers, and credentials and environment are not borrowed from Pi.
type StreamOptions = Pick<SimpleStreamOptions, "signal" | "onPayload" | "onProviderStreamEvent">;
type Wire = Record<string, unknown>;

export interface ToolExchange {
  tools: readonly Tool[];
  activate(sessionId: string, serverName: string): void;
  permit: AcpPermission;
  call: Parameters<typeof openAgyMcp>[1];
  stop(): void;
}

function exactSelection(configOptions: unknown, modelId: string, advertisedIds: ReadonlySet<string>): boolean | undefined {
  if (!Array.isArray(configOptions)) return;
  const value = (id: string) => record(configOptions.find(option => record(option)?.id === id))?.currentValue;
  const base = value("model"), effort = value("thought_level");
  // A split model/effort pair names an advertised variant even if its base is also advertised.
  if (typeof base === "string" && typeof effort === "string") {
    const variant = `${base}-${effort}`;
    if (advertisedIds.has(variant)) return variant === modelId;
  }
  const suffix = /-(low|medium|high|xhigh|max|minimal|off)$/.exec(modelId)?.[1];
  if (suffix !== undefined && effort !== undefined && effort !== suffix) return false;
  if (typeof base !== "string") return;
  if (base === modelId) return true;
  if (typeof effort === "string" || !modelId.startsWith(`${base}-`)) return false;
}

function updateFrom(wire: Wire): Wire | undefined {
  if (wire.method === "session/update" && !("id" in wire)) return record(record(wire.params)?.update);
}

function warningFrom(update: Wire | undefined): AcpFailure | undefined {
  if (update?.sessionUpdate !== "agent_message_chunk") return;
  const content = record(update.content);
  if (content?.type === "text" && typeof content.text === "string") return executorWarning(content.text);
}

function observableFrame(wire: Wire): Wire {
  if ("error" in wire) {
    const error = record(wire.error);
    return { jsonrpc: "2.0", id: Number.isSafeInteger(wire.id) ? wire.id : null, error: {
      code: Number.isSafeInteger(error?.code) ? error?.code : -32603,
      message: error ? rpcFailure(error).message : "agy ACP: invalid JSON-RPC error",
    } };
  }
  const update = updateFrom(wire);
  const warning = warningFrom(update);
  if (!warning) return wire;
  return { jsonrpc: "2.0", method: "session/update", params: {
    sessionId: record(wire.params)?.sessionId,
    update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: warning.message } },
  } };
}

export interface AgySession {
  peer: AcpPeer;
  sessionId: string;
  images: boolean;
  updateForSession(wire: Wire): Wire | undefined;
  checkUpdate(update: Wire): boolean;
}

export interface SessionHandoff {
  ready?: Promise<void>;
  reuse?: { session: AgySession; seed(): readonly AgyPromptBlock[] };
  /** Takes ownership before the terminal message is published to Pi. */
  retain?(session: AgySession, message: AssistantMessage): boolean;
}

async function openSession(peer: AcpPeer, modelId: string, instructions: string, observe: (wire: Wire) => Promise<void>,
  bridge?: AgyMcp["descriptor"]): Promise<AgySession> {
  const init = await peer.request("initialize", {
    protocolVersion: 1, clientInfo: { name: "corral-pi", version: "1" },
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
  }, observe);
  const capabilities = record(init.agentCapabilities);
  const meta = record(capabilities?._meta);
  if (init.protocolVersion !== 1 || record(record(meta?.antigravity)?.systemPrompt)?.append !== true) {
    throw new AcpFailure("protocol", "server does not support ACP v1 and role instructions");
  }
  const created = await peer.request("session/new", {
    cwd: peer.cwd, mcpServers: bridge ? [bridge] : [], _meta: { antigravity: { systemPrompt: { append: instructions } } },
  }, observe);
  const { sessionId } = created;
  if (typeof sessionId !== "string" || !sessionId) throw new AcpFailure("protocol", "missing session ID");
  const models = record(created.models);
  const unavailable = () => new AcpFailure("unavailable", "requested model is unavailable or not selected exactly");
  if (!Array.isArray(models?.availableModels)) throw unavailable();
  const advertisedIds = new Set(models.availableModels.map(item => record(item)?.modelId).filter(id => typeof id === "string"));
  if (!advertisedIds.has(modelId)) throw unavailable();
  const updateForSession = (wire: Wire) => record(wire.params)?.sessionId === sessionId ? updateFrom(wire) : undefined;
  const checkSelection = (configOptions: unknown) => {
    const exact = exactSelection(configOptions, modelId, advertisedIds);
    if (exact === false) throw unavailable();
    return exact === true;
  };
  const checkUpdate = (update: Wire) => update.sessionUpdate === "config_option_update" && checkSelection(update.configOptions);
  if (models.currentModelId === modelId) {
    checkSelection(created.configOptions);
  } else {
    let confirmed = false;
    const observeSelection = async (wire: Wire) => {
      await observe(wire);
      const update = updateForSession(wire);
      if (update && checkUpdate(update)) confirmed = true;
    };
    const selected = await peer.request("session/set_model", { sessionId, modelId }, observeSelection);
    if (checkSelection(selected.configOptions)) confirmed = true;
    if (selected.currentModelId !== undefined) {
      if (selected.currentModelId !== modelId) throw unavailable();
      confirmed = true;
    }
    // agy 1.5.14 acknowledges with {}. An ACK is not model evidence: the same session
    // publishes model + thought_level in config_option_update, even before session/prompt.
    await peer.updatesUntil(() => confirmed, observeSelection);
  }
  return { peer, sessionId, images: record(capabilities?.promptCapabilities)?.image === true, updateForSession, checkUpdate };
}

function initialMessage(model: Model<string>): AssistantMessage {
  return {
    role: "assistant", content: [], api: model.api, provider: model.provider, model: model.id,
    timestamp: Date.now(), stopReason: "stop",
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
  };
}

class ContentEvents {
  private index = -1;
  readonly stream = createAssistantMessageEventStream();
  readonly message: AssistantMessage;

  constructor(model: Model<string>) { this.message = initialMessage(model); }

  start(): void { this.stream.push({ type: "start", partial: this.message }); }

  tool(call: ToolCall): void {
    this.end();
    const partial = this.message;
    const contentIndex = partial.content.push(call) - 1;
    this.stream.push({ type: "toolcall_start", contentIndex, partial });
    this.stream.push({ type: "toolcall_delta", contentIndex, delta: JSON.stringify(call.arguments), partial });
    this.stream.push({ type: "toolcall_end", contentIndex, toolCall: call, partial });
    partial.stopReason = "toolUse";
    this.stream.push({ type: "done", reason: "toolUse", message: partial });
    this.stream.end();
  }

  update(update: Wire): void {
    const kind = update.sessionUpdate === "agent_message_chunk" ? "text" :
      update.sessionUpdate === "agent_thought_chunk" ? "thinking" : undefined;
    // Native agy tool events describe tools the agent owns; replaying them as Pi tool calls
    // would execute them twice, outside Pi's tool authority. Do not forward them.
    if (!kind) return;
    const content = record(update.content);
    if (content?.type !== "text" || typeof content.text !== "string") throw new AcpFailure("protocol", "invalid text chunk");
    const delta = content.text;
    if (!delta) return;
    const partial = this.message;
    if (partial.content[this.index]?.type !== kind) {
      this.end();
      this.index = partial.content.length;
      partial.content.push(kind === "text" ? { type: "text", text: "" } : { type: "thinking", thinking: "" });
      this.stream.push({ type: `${kind}_start`, contentIndex: this.index, partial });
    }
    const block = partial.content[this.index];
    if (block?.type === "text") block.text += delta;
    if (block?.type === "thinking") block.thinking += delta;
    this.stream.push({ type: `${kind}_delta`, contentIndex: this.index, delta, partial });
  }

  end(): void {
    const partial = this.message, contentIndex = this.index;
    const block = partial.content[contentIndex];
    if (block?.type === "text") this.stream.push({ type: "text_end", contentIndex, content: block.text, partial });
    if (block?.type === "thinking") this.stream.push({ type: "thinking_end", contentIndex, content: block.thinking, partial });
    this.index = -1;
  }
}

/** An ACP prompt is split at each MCP call: Pi finishes one assistant message and starts another. */
export class AgyOutput {
  current: ContentEvents;

  constructor(private readonly model: Model<string>) { this.current = new ContentEvents(model); }

  handoff(call: ToolCall): AssistantMessage {
    const previous = this.current;
    previous.tool(call);
    this.current = new ContentEvents(this.model);
    this.current.start();
    return previous.message;
  }
}

function applyUsage(message: AssistantMessage, usage: Wire | undefined): void {
  const count = (key: string) => {
    const value = usage?.[key];
    return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
  };
  const inputTokens = count("inputTokens"), outputTokens = count("outputTokens"),
    cachedReadTokens = count("cachedReadTokens"), cachedWriteTokens = count("cachedWriteTokens"),
    thoughtTokens = count("thoughtTokens"), totalTokens = count("totalTokens");
  // Pi requires numbers. Nulls in the diagnostic distinguish omissions from reported zero.
  // usage_update.used/size measure context occupancy, not per-turn usage. Do not price it.
  Object.assign(message.usage, { input: inputTokens ?? 0, output: outputTokens ?? 0, cacheRead: cachedReadTokens ?? 0,
    cacheWrite: cachedWriteTokens ?? 0, totalTokens: totalTokens ?? 0 });
  (message.diagnostics ??= []).push({ type: "agy_acp_usage", timestamp: Date.now(), details: {
    inputTokens, outputTokens, cachedReadTokens, cachedWriteTokens, thoughtTokens, totalTokens, costUsd: null,
  } });
}

function applyStop(message: AssistantMessage, reason: unknown): void {
  switch (reason) {
    case "end_turn": message.stopReason = "stop"; break;
    case "max_tokens": case "max_turn_requests": message.stopReason = "length"; break;
    case "cancelled": throw new AcpFailure("aborted", "turn cancelled");
    case "refusal": throw new AcpFailure("protocol", "agent refused the turn");
    default: throw new AcpFailure("protocol", "unknown stop reason");
  }
  message.rawStopReason = reason;
}

function applyFailure(message: AssistantMessage, failure: AcpFailure): void {
  message.stopReason = failure.category === "aborted" ? "aborted" : "error";
  message.errorMessage = failure.message;
  (message.diagnostics ??= []).push({ type: "agy_acp_failure", timestamp: Date.now(), details: {
    category: failure.category, ...(failure.source ? { source: failure.source } : {}),
  } });
}

/** Translate one text turn to Pi events without installing auth, model discovery or a worker route. */
export function streamAgy(launch: AgyLaunch, model: Model<string>, turn: AgyTurn, options?: StreamOptions): AssistantMessageEventStream {
  const output = new AgyOutput(model);
  const { stream } = output.current;
  void driveAgy(launch, model, turn, output, options);
  return stream;
}

export async function driveAgy(launch: AgyLaunch, model: Model<string>, turn: AgyTurn, output: AgyOutput,
  options?: StreamOptions, exchange?: ToolExchange, handoff?: SessionHandoff): Promise<void> {
  const reuse = handoff?.reuse;
  let peer = reuse?.session.peer;
  let session: AgySession | undefined;
  let retainable = handoff?.retain !== undefined && !exchange?.tools.length;
  const host: { bridge?: AgyMcp } = {};
  const lifetime = new AbortController();
  let active = true;
  let sessionId: string | undefined;
  let prompt: Promise<Wire> | undefined;
  let cancelMs = 0;
  let failure: AcpFailure | undefined;
  let warning: AcpFailure | undefined;
  let result: Wire | undefined;
  const signal = options?.signal;
  const observe = async (wire: Wire) => {
    if (active && options?.onProviderStreamEvent) await options.onProviderStreamEvent(observableFrame(wire), model);
  };
  try {
    await handoff?.ready;
    if (signal?.aborted) throw new AcpFailure("aborted", "turn cancelled");
    const setupMs = deadlineMs(launch.setupTimeoutMs, 15_000, 60_000);
    const turnMs = deadlineMs(launch.turnTimeoutMs, 300_000, 600_000);
    cancelMs = deadlineMs(launch.cancelTimeoutMs, 250, 2000);
    if (!turn.instructions.trim() || !model.id.trim()) throw new AcpFailure("protocol", "role instructions and exact model ID are required");
    const assertActive = () => {
      if (!active || signal?.aborted) throw new AcpFailure("aborted", "turn cancelled");
    };
    const setup = () => bounded(async () => {
      assertActive();
      const connection = peer = new AcpPeer(launch);
      if (exchange?.tools.length) {
        void connection.exited.then(() => exchange.stop());
        const mcpSignal = signal ? AbortSignal.any([signal, lifetime.signal]) : lifetime.signal;
        host.bridge = await openAgyMcp(exchange.tools, exchange.call, mcpSignal);
      }
      assertActive();
      return openSession(connection, model.id, turn.instructions, observe, host.bridge?.descriptor);
    }, setupMs, "session setup", signal);
    const initialSession = session = reuse?.session ?? await setup();
    const prepare = async (selected: AgySession, basePrompt: readonly AgyPromptBlock[]) => {
      if (selected === reuse?.session && !selected.peer.live) return;
      const payload = { sessionId: selected.sessionId, prompt: structuredClone(basePrompt) };
      const replacement = await options?.onPayload?.(payload, model);
      assertActive();
      if (selected === reuse?.session && !selected.peer.live) return;
      const edited = record(replacement === undefined ? payload : replacement);
      if (edited?.sessionId !== selected.sessionId || !Array.isArray(edited.prompt) || edited.prompt.length === 0) {
        throw new AcpFailure("protocol", "invalid prompt payload");
      }
      const blocks = edited.prompt.map(block => {
        const item = record(block);
        if (item?.type === "text" && typeof item.text === "string") return { type: "text", text: item.text };
        if (item?.type === "image" && typeof item.data === "string" && typeof item.mimeType === "string") {
          if (!selected.images) throw new AcpFailure("protocol", "session does not support image prompt content");
          return { type: "image", data: item.data, mimeType: item.mimeType };
        }
        throw new AcpFailure("protocol", "invalid prompt content block");
      });
      return { blocks, unchanged: isDeepStrictEqual(blocks, basePrompt) };
    };
    result = await bounded(async () => {
      let selectedSession = initialSession;
      const basePrompt: readonly AgyPromptBlock[] = typeof turn.prompt === "string" ? [{ type: "text", text: turn.prompt }] : turn.prompt;
      let prepared = await prepare(selectedSession, basePrompt);
      if (reuse && !selectedSession.peer.live) {
        // No session/prompt has been sent yet. Discard any edited delta and give the
        // payload hook the new session's Pi seed instead, within this same turn deadline.
        await selectedSession.peer.close();
        assertActive();
        const seed = reuse.seed();
        selectedSession = session = await setup();
        prepared = await prepare(selectedSession, seed);
      }
      assertActive();
      const connection = selectedSession.peer;
      if (!prepared || !connection.live) throw new AcpFailure("protocol", "session closed before the prompt");
      // A provider payload hook may edit what ACP sees without changing Pi's history.
      retainable &&= prepared.unchanged;
      connection.resume();
      sessionId = selectedSession.sessionId;
      output.current.start();
      if (host.bridge) exchange?.activate(selectedSession.sessionId, host.bridge.descriptor.name);
      const controls = host.bridge && exchange ? { permit: exchange.permit, beforeResponse: () => exchange.stop() } : undefined;
      prompt = connection.request("session/prompt", { sessionId: selectedSession.sessionId, prompt: prepared.blocks }, async wire => {
        if (!controls) await observe(wire);
        const update = active && !signal?.aborted ? selectedSession.updateForSession(wire) : undefined;
        if (update) {
          try { selectedSession.checkUpdate(update); }
          catch (error) { exchange?.stop(); throw error; }
          // Native tools add history Pi cannot account for. The session cannot be reused.
          if (update.sessionUpdate === "tool_call" || update.sessionUpdate === "tool_call_update") retainable = false;
          // Retain the sanitized warning, but drain to the terminal reply for usage.
          const executorFailure = warningFrom(update);
          warning ??= executorFailure;
          if (executorFailure) exchange?.stop();
          else output.current.update(update);
        }
        // Diagnostic consumers must not block later authority frames while Pi awaits approval.
        if (controls) void observe(wire).catch(() => undefined);
      }, controls);
      return await prompt;
    }, turnMs, "prompt", signal);
    if (signal?.aborted) throw new AcpFailure("aborted", "turn cancelled");
    applyStop(output.current.message, result.stopReason);
    failure = warning;
  } catch (error) {
    failure = error instanceof AcpFailure ? error : new AcpFailure("protocol", "turn failed");
  } finally {
    active = false;
    exchange?.stop();
    lifetime.abort();
    if (peer && sessionId && prompt && !result && (failure?.category === "aborted" || failure?.category === "timeout")) {
      const connection = peer, id = sessionId, pending = prompt;
      try {
        result = await bounded(async () => {
          await connection.notify("session/cancel", { sessionId: id });
          return pending;
        }, cancelMs, "cancellation");
      } catch { /* process-group cleanup does not require a cooperative agent */ }
    }
    const message = output.current.message;
    const retained = retainable && !failure && !signal?.aborted && message.stopReason === "stop" && session?.peer.live &&
      handoff?.retain?.(session, message);
    if (!retained) {
      try { await peer?.close(); }
      catch { failure = new AcpFailure("cleanup", "subprocess cleanup failed"); }
    }
    await host.bridge?.close();
  }
  const segment = output.current;
  const { message, stream } = segment;
  segment.end();
  applyUsage(message, record(result?.usage));
  if (failure) applyFailure(message, failure);
  switch (message.stopReason) {
    case "aborted": case "error": stream.push({ type: "error", reason: message.stopReason, error: message }); break;
    case "length": stream.push({ type: "done", reason: "length", message }); break;
    default: stream.push({ type: "done", reason: "stop", message }); break;
  }
  stream.end();
}
