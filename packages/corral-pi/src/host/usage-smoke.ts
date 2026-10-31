// T-570 bounded, explicitly invoked subscription smoke. Not a host workflow or comparison gate.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { z } from "zod";
import { Type, normalizeContext } from "@earendil-works/pi-ai";
import auth from "../auth/index.ts";
import { createModelCatalog } from "./catalog.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { authContext, restrictProvider } from "./session.ts";
import { sha256Hex } from "../tools/corral.ts";

const Profile = z.looseObject({
  role: z.enum(["worker", "reviewer"]), preset: z.enum(["strict", "standard"]),
  model: z.string(), prompt: z.string(),
  tools: z.array(z.object({ name: z.string(), description: z.string(), parameters: z.record(z.string(), z.unknown()) })),
});

export function smokeProfiles(raw: string) {
  const profiles = raw.split("\n").filter(line => line.startsWith("EARLY_STATIC "))
    .map(line => Profile.safeParse(JSON.parse(line.slice(13))))
    .flatMap(result => result.success ? [result.data] : []);
  if (profiles.length !== 4 || new Set(profiles.map(p => `${p.role}/${p.preset}`)).size !== 4) {
    throw new Error("expected exactly the four W/R strict/standard static profiles");
  }
  if (profiles.some(p => p.model !== "gpt-6.1-sol")) throw new Error("model control changed");
  return profiles;
}

export function cappedSmokePayload(payload: unknown) {
  const body = z.record(z.string(), z.unknown()).parse(payload);
  return { ...body, max_output_tokens: 1024 };
}

async function main() {
  const [log, out, permission] = process.argv.slice(2);
  if (!log || !out || permission !== "--authorized-T-570-Q1") {
    throw new Error("usage: usage-smoke.ts STATIC_LOG NEW_OUTPUT_DIR --authorized-T-570-Q1");
  }
  const raw = readFileSync(log, "utf8");
  const profiles = smokeProfiles(raw);
  // Exclusive directory: never overwrite prior admissions/results or silently repeat a failed run.
  mkdirSync(out);
  const task = "Do not call tools or claim to have executed code. Given input values [3, 1, 3, 2], return only JSON with sorted unique values and their sum, using keys values and sum.";
  const controls = { version: 1, partial: true, workflow_comparison: false,
    authorization: "T-570/R1/Q1 revision 2", provider: "openai-codex", model: "gpt-6.1-sol",
    requests_max: 4, max_output_tokens: 1024, max_retries: 0, paid_api_usd: 0,
    transport: "sse", effort: "none", order: profiles.map(p => `${p.role}/${p.preset}`),
    static_log_sha256: sha256Hex(raw), task,
    excluded: ["web", "MCP", "configurable LSP", "explore", "oracle", "native plan roles", "workflow outcomes"],
  };
  const save = (file: string, value: unknown) => writeFileSync(join(out, file), `${JSON.stringify(value, null, 2)}\n`, { flag: "wx" });
  save("controls.json", controls);
  restrictEnvironment(process.env, "openai-codex");
  const network = guardedFetch(globalThis.fetch.bind(globalThis));
  const models = await createModelCatalog(auth.credentials(authContext("openai-codex")), { providers: ["openai-codex"] });
  restrictProvider(models, "openai-codex", network.fetch as typeof globalThis.fetch);
  const model = models.getModel("openai-codex", controls.model);
  if (!model) throw new Error("authorized subscription model unavailable");
  let stopped = false;
  for (const [index, profile] of profiles.entries()) {
    const key = `${index + 1}-${profile.role}-${profile.preset}`;
    if (stopped) {
      save(`${key}-result.json`, { attempted: false, valid: false, outcome: "not_run_after_error" });
      continue;
    }
    save(`${key}-admission.json`, { profile, controls });
    const started = Date.now();
    const reply = await models.complete(model, normalizeContext({ systemPrompt: profile.prompt,
      messages: [{ role: "user", content: task, timestamp: started }],
      tools: profile.tools.map(tool => ({ ...tool, parameters: Type.Unsafe(tool.parameters) })),
    }), { maxTokens: 1024, maxRetries: 0, transport: "sse", reasoningEffort: "none",
      signal: AbortSignal.timeout(90_000),
      onPayload(payload: unknown) {
        // The Codex adapter does not forward maxTokens. Require the cap on the actual wire;
        // if the subscription backend refuses it, record a failed smoke, never retry uncapped.
        const capped = cappedSmokePayload(payload);
        save(`${key}-payload.json`, capped);
        return capped;
      },
    });
    const text = reply.content.flatMap(part => part.type === "text" ? [part.text] : []).join("");
    let valid = false;
    try {
      const value = z.object({ values: z.array(z.number()), sum: z.number() }).strict().parse(JSON.parse(text));
      valid = JSON.stringify(value.values) === "[1,2,3]" && value.sum === 6 && reply.stopReason === "stop"
        && reply.usage.output <= 1024;
    } catch { /* Raw output retained; failed format is not a success. */ }
    const uncertain = reply.stopReason === "error" || reply.stopReason === "aborted";
    save(`${key}-result.json`, { attempted: true, wall_ms: Date.now() - started, reply, valid,
      usage_status: uncertain ? "unknown_not_zero" : "reported",
      outcome: valid ? "smoke_valid" : "smoke_invalid", network: network.ledger() });
    console.log(JSON.stringify({ key, valid, stop_reason: reply.stopReason, usage: reply.usage }));
    stopped = uncertain;
  }
}
if (import.meta.main) main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
