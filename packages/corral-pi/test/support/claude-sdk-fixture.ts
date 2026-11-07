// The Claude Agent SDK lane (T-493) through the restricted Anthropic provider, against a fake SDK
// `query`: no Claude Code process, no network. The fake plays Claude Code's part: it streams the
// model's events and, for every tool call, asks the lane's hook and permission callback whether it
// may execute the tool itself.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { Options, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { type AssistantMessage, type Context, type Credential, normalizeContext } from "@earendil-works/pi-ai";

import { type SdkBoundary, overrideSdk } from "../../src/claude/sdk.ts";
import { guardedFetch } from "../../src/host/restrict.ts";
import { restrictProvider } from "../../src/host/session.ts";

export const OAUTH = "sk-ant-oat01-SYNTHETIC-SUBSCRIPTION";
export const PAID = "sk-ant-api03-SYNTHETIC-PAID-KEY";
const temps: string[] = [];

export function resetSdkFixture(): void {
  overrideSdk(undefined);
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
}

type Script = (options: Options, control: { interrupted: Promise<void> }) => AsyncGenerator<unknown>;

interface FakeRun {
  options: Options[];
  prompts: SDKUserMessage[];
  interrupts: number;
  closes: number;
}

export function fakeSdk(script: Script): FakeRun {
  const run: FakeRun = { options: [], prompts: [], interrupts: 0, closes: 0 };
  const boundary: SdkBoundary = {
    query: ({ prompt, options }) => {
      run.options.push(options);
      let interrupted!: () => void;
      const control = { interrupted: new Promise<void>((resolve) => { interrupted = resolve; }) };
      const generator = (async function* () {
        for await (const message of prompt) run.prompts.push(message);
        yield* script(options, control);
      })();
      return Object.assign(generator, {
        interrupt: async () => {
          run.interrupts++;
          interrupted();
          return undefined;
        },
        close: () => { run.closes++; },
      }) as never;
    },
    createSdkMcpServer: (options) => ({ type: "sdk", name: options.name, instance: options }) as never,
  };
  overrideSdk(boundary);
  return run;
}

export async function anthropic(env: NodeJS.ProcessEnv = { PATH: "/bin" }) {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-claude-sdk-"));
  temps.push(root);
  const credential: Credential = { type: "oauth", access: OAUTH, refresh: "r", expires: Date.now() + 3_600_000 };
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: {
      async read(provider: string) { return provider === "anthropic" ? credential : undefined; },
      async list() { return [{ providerId: "anthropic", type: "oauth" as const }]; },
      async modify() { throw new Error("read-only"); },
      async delete() { throw new Error("read-only"); },
    },
    modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  const guard = guardedFetch(async () => new Response("unexpected", { status: 500 }), undefined, "anthropic");
  const configDir = join(root, "claude-code");
  restrictProvider(models, "anthropic", guard.fetch as typeof fetch,
    { configDir, cwd: root, env, executable: () => "/synthetic/claude" });
  const model = models.getModel("anthropic", "claude-sonnet-5-5")!;
  const provider = models.getProvider("anthropic")!;
  const send = async (context: Context, options: Record<string, unknown> = {}) => {
    const auth = await models.getAuth("anthropic");
    const stream = provider.streamSimple(model, normalizeContext(context),
      { apiKey: auth?.auth.apiKey, ...options } as never);
    const events: string[] = [];
    for await (const event of stream) events.push(event.type);
    return { events, message: await stream.result() as AssistantMessage };
  };
  return { send, guard, configDir };
}

export const user = (text: string) => ({ role: "user" as const, content: text, timestamp: Date.now() });

export function* textReply(text: string) {
  yield { type: "stream_event", event: { type: "message_start", message: { usage: { input_tokens: 12, output_tokens: 0 } } } };
  yield { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } };
  yield { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: text.slice(0, 3) } } };
  yield { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: text.slice(3) } } };
  yield { type: "stream_event", event: { type: "content_block_stop", index: 0 } };
  yield { type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } } };
  yield { type: "result", subtype: "success", is_error: false, result: text, stop_reason: "end_turn", usage: { input_tokens: 12, output_tokens: 5 } };
}
