// Offline reanalysis only. Reads raw records, never repeats inference or rewrites captures.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { apiEquivalent, digest, dynamicCohort, profileKey, validateProfile } from "../src/capability-profile.ts";
const [statics, smoke, output] = process.argv.slice(2);
if (!statics || !smoke || !output) throw new Error("usage: bun capture/audit.ts STATIC_DIR SMOKE_DIR NEW_REPORT_JSON");
const load = (directory: string, path: string) => JSON.parse(readFileSync(join(directory, path), "utf8"));
const profiles = [];
const startup = [];
for (const resource of ["inert", "configured"]) for (const role of ["worker", "reviewer", "pm", "planner", "plan_reviewer"]) {
  for (const preset of ["strict", "standard"]) {
    const key = `${resource}-${role}-${preset}`;
    const p = validateProfile(load(statics, `${key}.json`));
    startup.push(p);
    profiles.push(load(statics, `${key}${resource === "configured" && preset === "standard" ? "-activated" : ""}.json`));
  }
}
for (const p of startup.filter(p => p.preset === "strict" && p.runnable)) {
  const counterpart = startup.find(other => other.role === p.role && other.resource !== p.resource && other.preset === "strict");
  if (counterpart?.prompt_sha256 !== p.prompt_sha256 || counterpart?.tools_sha256 !== p.tools_sha256) throw new Error("strict resource controls differ");
}
const cohort = dynamicCohort(profiles);
const controls = load(smoke, "controls.json");
if (digest(JSON.stringify(profiles)) !== controls.profiles_sha256 || JSON.stringify(cohort.map(profileKey)) !== JSON.stringify(controls.order)) {
  throw new Error("preregistered controls changed");
}
const ids = new Set();
const rows = cohort.map((profile, index) => {
  const key = `${String(index + 1).padStart(2, "0")}-${profileKey(profile)}`;
  const result = load(smoke, `${key}-result.json`);
  if (!result.attempted) return { key, ...result, api_equivalent_usd: null };
  const payload = load(smoke, `${key}-payload.json`);
  if (payload.max_output_tokens !== 1024) throw new Error("wire cap changed");
  const reply = result.reply;
  if (!reply) return { key, ...result, api_equivalent_usd: null };
  if (ids.has(reply.responseId)) throw new Error("duplicate response identity");
  ids.add(reply.responseId);
  const text = reply.content.filter((p: { type: string }) => p.type === "text").map((p: { text: string }) => p.text).join("");
  let valid = false;
  try {
    const answer = JSON.parse(text);
    valid = Object.keys(answer).sort().join(",") === "sum,values" && JSON.stringify(answer.values) === "[1,2,3]" && answer.sum === 6 &&
      reply.stopReason === "stop" && reply.usage.output <= 1024 && !reply.content.some((p: { type: string }) => p.type === "toolCall");
  } catch { /* Invalid outcomes stay invalid. */ }
  if (valid !== result.valid) throw new Error("raw outcome validity disagrees with stored result");
  const uncertain = ["error", "aborted"].includes(reply.stopReason);
  return { key, role: profile.role, resource: profile.resource, preset: profile.preset, attempted: true, valid,
    wall_ms: result.wall_ms, usage: reply.usage, response_id: reply.responseId,
    api_equivalent_usd: apiEquivalent(reply.usage, uncertain), raw_api_equivalent_usd: result.api_equivalent_usd };
});
const inputs = [statics, smoke].flatMap((directory, index) => readdirSync(directory).sort().map(file => ({
  path: `${index === 0 ? "static" : "smoke"}/${file}`, sha256: digest(readFileSync(join(directory, file), "utf8")),
})));
const summary = { controls, static_profiles: startup.map(p => ({ key: profileKey(p), runnable: p.runnable,
  prompt_tokens_estimate: p.prompt_tokens_estimate, tool_tokens_estimate: p.tool_tokens_estimate,
  prompt_sha256: p.prompt_sha256, tools_sha256: p.tools_sha256 })), rows, inputs,
  analysis_note: "Raw API-equivalent fields are null due to initial SDK nested-cost validation bug; recomputed from preserved reported usage with explicit input/output/cache fields. No inference rerun.",
  workflow_comparison: false };
writeFileSync(output, JSON.stringify(summary, null, 2) + "\n", { flag: "wx" });
console.log(JSON.stringify({ static_trials: startup.length, dynamic_trials: rows.length, valid: rows.filter(row => row.valid).length,
  attempted: rows.filter(row => row.attempted).length, workflow_comparison: false }));
