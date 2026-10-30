// Explicitly authorized main-request cost smoke only; never a workflow quality experiment.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Type, normalizeContext } from "../../../packages/corral-pi/node_modules/@earendil-works/pi-ai";
import auth from "../../../packages/corral-pi/src/auth/index.ts";
import { createModelCatalog } from "../../../packages/corral-pi/src/host/catalog.ts";
import { guardedFetch, restrictEnvironment } from "../../../packages/corral-pi/src/host/restrict.ts";
import { authContext, restrictProvider } from "../../../packages/corral-pi/src/host/session.ts";
import { apiEquivalent, digest, dynamicCohort, profileKey } from "../src/capability-profile.ts";

const [directory, output, authorization] = process.argv.slice(2);
if (!directory || !output || authorization !== "--authorized-T-579-R1-Q1") {
  throw new Error("usage: bun capture/smoke.ts STATIC_DIR NEW_OUTPUT_DIR --authorized-T-579-R1-Q1 (requires fresh owner permission)");
}
const raw = [];
for (const resource of ["inert", "configured"]) for (const role of ["worker", "reviewer", "pm", "planner", "plan_reviewer"]) {
  for (const preset of ["strict", "standard"]) {
    const activated = resource === "configured" && preset === "standard" ? "-activated" : "";
    raw.push(JSON.parse(readFileSync(join(directory, `${resource}-${role}-${preset}${activated}.json`), "utf8")));
  }
}
const cohort = dynamicCohort(raw);
mkdirSync(output); // Exclusive, no retries/overwriting after any failure.
const save = (file: string, value: unknown) => writeFileSync(join(output, file), JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
const task = "Do not call tools or claim to have executed code. Given input values [3, 1, 3, 2], return only JSON with sorted unique values and their sum, using keys values and sum.";
const controls = { version: 1, authorization: "T-579/R1/Q1 revision 2", provider: "openai-codex", model: "gpt-6.1-sol",
  runner_sha256: digest(readFileSync(import.meta.filename, "utf8")),
  requests_max: 16, max_output_tokens: 1024, max_retries: 0, paid_api_usd: 0, effort: "none", transport: "sse",
  task, order: cohort.map(profileKey), profiles_sha256: digest(JSON.stringify(raw)),
  workflow_comparison: false, tools_executed: false, auxiliary_requests: 0, summaries: 0,
  source_controls: JSON.parse(readFileSync(join(directory, "controls.json"), "utf8")) };
save("controls.json", controls);
restrictEnvironment(process.env, "openai-codex");
const network = guardedFetch(globalThis.fetch.bind(globalThis));
const models = await createModelCatalog(auth.credentials(authContext("openai-codex")), { providers: ["openai-codex"] });
restrictProvider(models, "openai-codex", network.fetch as typeof globalThis.fetch);
const model = models.getModel("openai-codex", controls.model);
if (!model) throw new Error("authorized subscription model unavailable");
let stopped = false;
for (const [index, p] of cohort.entries()) {
  const key = `${String(index + 1).padStart(2, "0")}-${profileKey(p)}`;
  if (stopped) {
    save(`${key}-result.json`, { attempted: false, valid: false, outcome: "not_run_after_error", api_equivalent_usd: null });
    continue;
  }
  save(`${key}-admission.json`, { profile: p, controls });
  const started = Date.now();
  let payloadCount = 0;
  try {
    const reply = await models.complete(model, normalizeContext({ systemPrompt: p.prompt!,
      messages: [{ role: "user", content: task, timestamp: started }],
      tools: p.tools!.map(tool => ({ ...tool, parameters: Type.Unsafe(tool.parameters) })),
    }), { maxTokens: 1024, maxRetries: 0, transport: "sse", reasoningEffort: "none", signal: AbortSignal.timeout(90_000),
      onPayload(payload: unknown) {
        if (++payloadCount > 1) throw new Error("automatic request repeat refused");
        const capped = { ...(payload as object), max_output_tokens: 1024 };
        save(`${key}-payload.json`, capped);
        return capped;
      },
    });
    const text = reply.content.flatMap(part => part.type === "text" ? [part.text] : []).join("");
    let valid = false;
    try {
      const value = JSON.parse(text);
      valid = JSON.stringify(value.values) === "[1,2,3]" && value.sum === 6 &&
        Object.keys(value).sort().join(",") === "sum,values" && reply.stopReason === "stop" && reply.usage.output <= 1024 &&
        !reply.content.some(part => part.type === "toolCall");
    } catch { /* Full reply retained, never promote malformed output. */ }
    const uncertain = ["error", "aborted"].includes(reply.stopReason);
    save(`${key}-result.json`, { attempted: true, wall_ms: Date.now() - started, reply, valid,
      usage_status: uncertain ? "unknown_not_zero" : "reported", api_equivalent_usd: apiEquivalent(reply.usage, uncertain),
      outcome: valid ? "smoke_valid" : "smoke_invalid", network: network.ledger() });
    stopped = uncertain || reply.usage.output > 1024;
    console.log(JSON.stringify({ key, valid, stop_reason: reply.stopReason, usage: reply.usage }));
  } catch (error) {
    save(`${key}-result.json`, { attempted: payloadCount > 0, wall_ms: Date.now() - started, valid: false,
      usage_status: "unknown_not_zero", api_equivalent_usd: null, outcome: "exception", error: String(error) });
    stopped = true;
  }
}
