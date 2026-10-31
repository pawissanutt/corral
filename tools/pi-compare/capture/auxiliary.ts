// Offline auxiliary overhead capture through the production auxiliary SDK adapter.
// Synthetic responses/usage are not provider measurements or spend.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Type } from "../../../packages/corral-pi/node_modules/@earendil-works/pi-ai";
import type { ToolDefinition } from "../../../packages/corral-pi/node_modules/@earendil-works/pi-coding-agent";
import auth from "../../../packages/corral-pi/test/fixtures/auth/index.ts";
import { createModelCatalog } from "../../../packages/corral-pi/src/host/catalog.ts";
import { guardedFetch, restrictEnvironment } from "../../../packages/corral-pi/src/host/restrict.ts";
import { authContext, restrictProvider } from "../../../packages/corral-pi/src/host/session.ts";
import { runAuxiliarySession } from "../../../packages/corral-pi/src/extensions/subagent/session.ts";
import { digest, validateProfile } from "../src/capability-profile.ts";

const [directory, output] = process.argv.slice(2);
if (!directory || !output) throw new Error("usage: bun capture/auxiliary.ts STATIC_DIR NEW_OUTPUT_DIR");
mkdirSync(output);
const work = mkdtempSync(join(tmpdir(), "corral-final-auxiliary-"));
restrictEnvironment(process.env);
const network = guardedFetch(auth.transport!(authContext()));
const models = await createModelCatalog(auth.credentials(authContext()), { providers: ["openai-codex"] });
restrictProvider(models, "openai-codex", network.fetch as typeof globalThis.fetch);
const provider = models.getProvider("openai-codex");
if (!provider) throw new Error("fixture provider missing");
const sdk = await import("../../../packages/corral-pi/node_modules/@earendil-works/pi-coding-agent");
let captured: unknown;
models.registerNativeProvider({ ...provider,
  stream: (current, context, options) => { captured = context; return provider.stream(current, context, options); },
  streamSimple: (current, context, options) => { captured = context; return provider.streamSimple(current, context, options); },
});
try {
  const scenarios = (["inert", "configured"] as const).flatMap(resource =>
    (["worker", "planner", "plan_reviewer"] as const).map(role => ({ resource, role, kind: "explore" as const })));
  // Oracle is full/override only. Include a reference overhead, never call it standard-enabled.
  for (const scenario of [...scenarios, { resource: "inert", role: "worker", kind: "oracle" } as const]) {
    const { resource, role, kind } = scenario;
    const p = validateProfile(JSON.parse(readFileSync(join(directory, `${resource}-${role}-standard.json`), "utf8")));
    const key = `${resource}-${role}-${kind}`;
    captured = undefined;
    const localTools: ToolDefinition[] = p.tools!.filter(tool => /^(lsp_|web_)/.test(tool.name)).map(tool => ({
      ...tool, label: tool.name, parameters: Type.Unsafe(tool.parameters),
      async execute() { throw new Error("auxiliary static capture must never execute a tool"); },
    }));
    const result = await runAuxiliarySession({ sdk, models, provider: "openai-codex", cwd: work, agentDir: work,
      parentLoadout: p.tools!.map(tool => tool.name), localTools }, {
      request: key, kind, model: "gpt-6.1-sol", prompt: "Return concise fixture findings. Do not call tools.",
      sessionFile: join(work, `${key}.jsonl`), sessionId: key, maxTurns: 1,
    }, AbortSignal.timeout(15_000));
    const context = JSON.parse(JSON.stringify(captured).split(work).join("<fixture>"));
    const system = context.messages.find((message: { role: string }) => message.role === "system");
    if (!system) throw new Error("auxiliary context missing system message");
    const prompt = Object.values(system.sections ?? { content: system.content }).join("\n\n");
    const tools = (system.toolsAdded ?? []).map(({ name, description, parameters }: ToolDefinition) => ({ name, description, parameters }));
    const toolJson = JSON.stringify(tools);
    writeFileSync(join(output, `${key}.json`), JSON.stringify({ scenario, inference: false, synthetic_response: true,
      standard_enabled: kind === "explore", result, context, estimator: "utf8-json-bytes/4-v1",
      prompt_bytes: Buffer.byteLength(prompt), tool_bytes: Buffer.byteLength(toolJson),
      prompt_tokens_estimate: Math.ceil(Buffer.byteLength(prompt) / 4), tool_tokens_estimate: Math.ceil(Buffer.byteLength(toolJson) / 4),
      prompt_sha256: digest(prompt), tools_sha256: digest(toolJson), dynamic_api_equivalent_usd: null,
    }, null, 2) + "\n", { flag: "wx" });
    console.log(JSON.stringify({ key, result, tools: tools.map((tool: { name: string }) => tool.name) }));
  }
} finally { rmSync(work, { recursive: true, force: true }); }
