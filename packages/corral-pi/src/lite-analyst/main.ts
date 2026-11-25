import { createHash } from "node:crypto";
import { Agent } from "@earendil-works/pi-agent-core";
import type { AgentTool, StreamFn } from "@earendil-works/pi-agent-core";
import { createAssistantMessageEventStream, getCurrentSystemPrompt, getCurrentTools } from "@earendil-works/pi-ai";
import type { AssistantMessage, JsonObject, Model, ToolCall, TranscriptContext } from "@earendil-works/pi-ai";
import { compile } from "@earendil-works/pi-schema";
import { negotiateEvidence } from "./mcp.ts";
import { canonical, closedObject, FixturePipe, isObject, MAX_BODY_BYTES, parseStrictJson, requireWire } from "./wire.ts";

// C1/T-2069 asset pins, independent of the Rust supervisor and any launch-supplied manifest.
// The policy pin here hashes J(tool-policy.json.model_tools) only, with its array order intact.
const PROMPT_SHA256 = "103ccacd5a80abbd9f83e4d64441cbb6b07e5d7a8c28c793d15bc16aaded8f02";
const MODEL_TOOLS_SHA256 = "42123fb56609d54fc50fe23578df53f17f825fa8ad35c973581c2698f793a435";
const MAX_TURNS = 4;
const MAX_EVIDENCE_CALLS = 8;
const MAX_EVIDENCE_BYTES = 8192;
const MAX_TOTAL_EVIDENCE_BYTES = 32_768;
const FINISH = "finish_report";

// Metadata for the explicit Agent stream function, not a registered provider/model catalog.
// Only Rust knows the real provider endpoint, credential, meters and permits.
const MODEL: Model<"lite-analyst-stdio"> = {
  id: "fixture-v1",
  name: "fixture-v1",
  api: "lite-analyst-stdio",
  provider: "scripted",
  baseUrl: "",
  input: ["text"],
  reasoning: false,
  contextWindow: MAX_BODY_BYTES,
  maxTokens: 0,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

interface Call {
  id: string;
  name: string;
  raw: string;
  args: JsonObject;
}

function sha256(text: string): string {
  return createHash("sha256").update(text).digest("hex");
}

async function run(pipe: FixturePipe): Promise<void> {
  const start = closedObject(await pipe.receive(), "type", "question", "prompt", "tools");
  const { prompt, question } = start;
  requireWire(start.type === "start" && typeof question === "string" && Buffer.byteLength(question) <= 4096);
  requireWire(typeof prompt === "string" && sha256(prompt) === PROMPT_SHA256 && Array.isArray(start.tools) &&
    sha256(canonical(start.tools)) === MODEL_TOOLS_SHA256);

  let turns = 0;
  let evidenceCalls = 0;
  let evidenceBytes = 0;
  let failed = false;
  let finishAcknowledged = false;
  let completed = false;
  let pending: Call[] = [];
  let nextCall = 0;
  const calls = new Map<string, Call>();
  const validators = new Map<string, (args: unknown) => boolean>();

  // No coding-agent session: it would add discovery, persistence, or automatic retries.
  // These four Pi AgentTool execute callbacks are the only dispatch path in the process.
  const tools: AgentTool[] = start.tools.map((value: unknown): AgentTool => {
    const { name, description, inputSchema } = closedObject(value, "name", "description", "inputSchema");
    requireWire(typeof name === "string" && typeof description === "string" && isObject(inputSchema));
    const validate = compile(inputSchema, { coerce: false }).check;
    validators.set(name, validate);
    return {
      name,
      label: name,
      description,
      parameters: inputSchema,
      execute: async (id, args, signal) => {
        try {
          const expected = pending[nextCall];
          requireWire(!failed && !finishAcknowledged && !signal?.aborted && expected?.id === id &&
            expected.name === name && isObject(args) && validate(args) && canonical(args) === canonical(expected.args));
          await pipe.send({ type: "tool", id, name, args });
          const result = closedObject(await pipe.receive(), "type", "content");
          requireWire(!signal?.aborted && result.type === "result" && typeof result.content === "string");
          const content = result.content;
          if (name === FINISH) {
            const receipt = closedObject(parseStrictJson(content), "offer");
            const offer = closedObject(receipt.offer, "corral_instance", "run_id", "generation", "report_id", "sha256", "bytes");
            requireWire([offer.corral_instance, offer.run_id, offer.report_id].every(
              (value) => typeof value === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(value),
            ) && typeof offer.sha256 === "string" && /^[0-9a-f]{64}$/.test(offer.sha256) &&
              typeof offer.generation === "number" && Number.isSafeInteger(offer.generation) && offer.generation > 0 &&
              typeof offer.bytes === "number" && Number.isSafeInteger(offer.bytes) && offer.bytes > 0 && offer.bytes <= 65536 &&
              canonical(receipt) === content);
            finishAcknowledged = true;
          } else {
            const bytes = Buffer.byteLength(content);
            requireWire(bytes <= MAX_EVIDENCE_BYTES && evidenceBytes + bytes <= MAX_TOTAL_EVIDENCE_BYTES);
            const payload = parseStrictJson(content);
            // The supervisor validates the reply schema and substitutes terminal host polls.
            // Keep the exact canonical string, not the SDK's or our own reserialization.
            requireWire(isObject(payload) && (payload.status === "ok" || payload.status === "error") &&
              canonical(payload) === content);
            evidenceBytes += bytes;
          }
          nextCall++;
          return { content: [{ type: "text", text: content }], details: {} };
        } catch {
          // Pi catches tool errors; fence subsequent callbacks and turn scheduling ourselves.
          failed = true;
          throw new Error("closed fixture tool failed");
        }
      },
    };
  });

  function render(context: TranscriptContext) {
    const [system, user, ...history] = context.messages;
    const renderedPrompt = getCurrentSystemPrompt(context.messages);
    requireWire(system?.role === "system" && renderedPrompt === prompt &&
      user?.role === "user" && user.content === question);
    const registry = getCurrentTools(context.messages).map((tool) => {
      requireWire(isObject(tool.parameters) && tool.constrainedSampling === undefined);
      return { name: tool.name, description: tool.description, inputSchema: tool.parameters };
    });
    requireWire(sha256(canonical(registry)) === MODEL_TOOLS_SHA256);
    const messages = [
      { role: "system", content: renderedPrompt },
      { role: "user", content: user.content },
      ...history.map((message) => {
        if (message.role === "assistant") {
          requireWire(message.stopReason === "toolUse" && message.content.length > 0);
          return {
            role: "assistant",
            content: null,
            tool_calls: message.content.map((block) => {
              requireWire(block.type === "toolCall");
              const call = calls.get(block.id);
              requireWire(call && call.name === block.name && canonical(call.args) === canonical(block.arguments));
              return { id: call.id, type: "function", function: { name: call.name, arguments: call.raw } };
            }),
          };
        }
        requireWire(message.role === "toolResult" && !message.isError && message.content.length === 1);
        const block = message.content[0];
        requireWire(block?.type === "text" && calls.get(message.toolCallId)?.name === message.toolName);
        return { role: "tool", tool_call_id: message.toolCallId, content: block.text };
      }),
    ];
    const body = {
      model: MODEL.id,
      stream: true,
      messages,
      tools: registry.map(({ name, description, inputSchema }) => ({
        type: "function", function: { name, description, parameters: inputSchema },
      })),
    };
    requireWire(Buffer.byteLength(JSON.stringify(body)) <= MAX_BODY_BYTES);
    return body;
  }

  function providerCalls(message: JsonObject): Call[] {
    closedObject(message, "role", "content", "tool_calls");
    requireWire(message.content === null && Array.isArray(message.tool_calls) && message.tool_calls.length > 0 &&
      message.tool_calls.length <= MAX_EVIDENCE_CALLS + 1);
    const ids = new Set<string>();
    // Validate the *whole* turn before exposing any call to Pi. Its usual provider/SDK
    // parsers can repair partial JSON and coerce arguments, neither of which C1 permits.
    return message.tool_calls.map((value: unknown, index: number, batch: unknown[]): Call => {
      const call = closedObject(value, "id", "type", "function");
      const { id } = call;
      const { name, arguments: raw } = closedObject(call.function, "name", "arguments");
      requireWire(typeof id === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(id) && !ids.has(id) && !calls.has(id) &&
        call.type === "function" && typeof name === "string" && typeof raw === "string" &&
        Buffer.byteLength(raw) <= MAX_BODY_BYTES);
      const args = parseStrictJson(raw);
      requireWire(isObject(args) && validators.get(name)?.(args));
      if (name === FINISH) requireWire(index === batch.length - 1);
      else requireWire(++evidenceCalls <= MAX_EVIDENCE_CALLS);
      ids.add(id);
      return { id, name, raw, args };
    });
  }

  const streamFn: StreamFn = async (model, context, options) => {
    const stream = createAssistantMessageEventStream();
    const message: AssistantMessage = {
      role: "assistant", model: MODEL.id, api: MODEL.api, provider: MODEL.provider,
      content: [], stopReason: "stop", timestamp: Date.now(),
      // Only the Rust transport accounts for the synthetic fixture tariff.
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
        cost: { ...MODEL.cost, total: 0 } },
    };
    try {
      requireWire(!failed && !finishAcknowledged && !options?.signal?.aborted && model === MODEL && turns < MAX_TURNS);
      const body = render(context);
      turns++;
      await pipe.send({ type: "turn", body });
      const response = closedObject(await pipe.receive(), "type", "message");
      const assistant = response.message;
      requireWire(!options?.signal?.aborted && response.type === "provider" && isObject(assistant) &&
        assistant.role === "assistant");
      if (typeof assistant.content === "string") {
        closedObject(assistant, "role", "content");
        pending = [];
        nextCall = 0;
        stream.push({ type: "start", partial: message });
        const text = { type: "text" as const, text: "" };
        message.content.push(text);
        stream.push({ type: "text_start", contentIndex: 0, partial: message });
        text.text = assistant.content;
        stream.push({ type: "text_delta", contentIndex: 0, delta: text.text, partial: message });
        stream.push({ type: "text_end", contentIndex: 0, content: text.text, partial: message });
      } else {
        pending = providerCalls(assistant);
        nextCall = 0;
        message.stopReason = "toolUse";
        stream.push({ type: "start", partial: message });
        for (const [contentIndex, call] of pending.entries()) {
          calls.set(call.id, call);
          const block: ToolCall = { type: "toolCall", id: call.id, name: call.name, arguments: {} };
          message.content.push(block);
          stream.push({ type: "toolcall_start", contentIndex, partial: message });
          block.arguments = call.args;
          stream.push({ type: "toolcall_delta", contentIndex, delta: call.raw, partial: message });
          stream.push({ type: "toolcall_end", contentIndex, toolCall: block, partial: message });
        }
      }
      const reason = pending.length > 0 ? "toolUse" : "stop";
      stream.push({ type: "done", reason, message });
    } catch {
      failed = true;
      const reason = options?.signal?.aborted ? "aborted" : "error";
      message.stopReason = reason;
      message.errorMessage = "closed fixture turn failed";
      stream.push({ type: "error", reason, error: message });
    }
    stream.end();
    return stream;
  };

  const agent = new Agent({
    initialState: { systemPrompt: prompt, tools, model: MODEL, thinkingLevel: "off" },
    identity: { attribution: null },
    streamFn,
    toolExecution: "sequential",
    finishTurn: async ({ message, toolResults }) => {
      if (message.stopReason !== "toolUse" || nextCall !== pending.length || toolResults.length !== pending.length ||
          toolResults.some((result) => result.isError)) failed = true;
      if (failed || finishAcknowledged) {
        completed = !failed && finishAcknowledged;
        return { action: "end" };
      }
    },
  });
  // The first three entries of the ordered, hash-pinned registry are the evidence tools.
  // finish_report is local to the host adapter and is not part of the shim registry.
  await negotiateEvidence(pipe, start.tools.slice(0, 3));
  await agent.prompt({ role: "user", content: question, timestamp: Date.now() });
  requireWire(completed && !agent.state.errorMessage);
  await pipe.send({ type: "done" });
}

async function main(): Promise<void> {
  // The supervisor enforces the actual (possibly shorter) deadline and owns teardown.
  // This independent ceiling also bounds a stalled stdio peer.
  const timer = setTimeout(() => {
    process.stdin.destroy();
    process.stdout.destroy();
  }, 120_000);
  process.stdout.on("error", () => { /* A failed write rejects the current turn. */ });
  try {
    requireWire(process.argv.length === 2);
    await run(new FixturePipe(process.stdin, process.stdout));
  } catch {
    process.exitCode = 1;
  } finally {
    clearTimeout(timer);
    process.stdin.destroy();
  }
}

await main();
