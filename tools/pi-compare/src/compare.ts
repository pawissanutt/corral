import { readFile, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { aggregate, measure, selectedRun } from "./metrics.ts";
import {
  ComparisonError, assignedOrder, canonical, manifestSchema, parseEvents, selectionSchema, sha256, showSchema,
} from "./schema.ts";
import type { Manifest, Pair, Show } from "./schema.ts";
import type { Metrics } from "./metrics.ts";

export async function readRaw(path: string): Promise<string> {
  const info = await stat(path);
  if (!info.isFile() || info.size > 32 * 1024 * 1024) {
    throw new ComparisonError("raw_file_bounds", path);
  }
  return readFile(path, "utf8");
}

export function validatePair(pair: Pair, seed: string, omo: Show, pi: Show) {
  const errors: string[] = [];
  if (omo.id !== pi.id || omo.body !== pi.body || omo.kind !== pi.kind || omo.size !== pi.size) {
    errors.push("task/spec/size/kind differs");
  }
  if (canonical(pair.order) !== canonical(assignedOrder(seed, pair.id))) errors.push("order differs from seeded assignment");
  if (canonical(pair.omo.before) !== canonical(pair.omo.after)
    || canonical(pair.pi_native.before) !== canonical(pair.pi_native.after)
    || canonical(pair.omo.before) !== canonical(pair.pi_native.before)) {
    errors.push("binary/config/contract/reviewer/PM/slot identity drift");
  }
  for (const [show, arm, harness] of [
    [omo, pair.omo, "omo"], [pi, pair.pi_native, "pi-native"],
  ] as const) {
    const run = selectedRun(show, arm.run);
    if (run.harness !== harness) errors.push(`${harness}: harness mismatch`);
    const model = run.model.replace(/^(chatgpt-subscription|openai-codex)\//, "");
    if (model !== "gpt-6.1-sol") errors.push(`${harness}: model is not frozen gpt-6.1-sol`);
    if (sha256(canonical(show.gate_contract)) !== arm.before.gate_contract_sha256) {
      errors.push(`${harness}: gate contract digest mismatch`);
    }
    if (run.review?.reviewer !== arm.before.reviewer_profile
      || run.review_runs.some((child) => child.agent !== arm.before.reviewer_profile)) {
      errors.push(`${harness}: independent reviewer profile mismatch`);
    }
  }
  const left = selectedRun(omo, pair.omo.run);
  const right = selectedRun(pi, pair.pi_native.run);
  if (left.id === right.id) errors.push("worker execution reused across arms");
  if (left.base_sha !== right.base_sha) errors.push("base differs");
  if (left.effort !== right.effort) errors.push("effort differs");
  const first = pair.order[0] === "omo" ? left : right;
  const second = pair.order[0] === "omo" ? right : left;
  if (Date.parse(first.started) >= Date.parse(second.started)) errors.push("observed order differs");
  return errors;
}

function thresholds(omo: ReturnType<typeof aggregate>, pi: ReturnType<typeof aggregate>) {
  return {
    needs_me: pi.needs_me <= omo.needs_me,
    turns_ended_without_report: pi.turns_ended_without_report <= omo.turns_ended_without_report,
    review_rounds: pi.review_rounds <= omo.review_rounds + 1,
    api_equivalent_cost: omo.api_equivalent_usd === null || pi.api_equivalent_usd === null
      ? null : pi.api_equivalent_usd <= 1.25 * omo.api_equivalent_usd,
    gate_pass_rate: omo.gate_pass_rate === null || pi.gate_pass_rate === null
      ? null : pi.gate_pass_rate >= omo.gate_pass_rate,
  };
}

export async function compare(manifest: Manifest, root: string) {
  const used = new Set<string>();
  const pairs: {
    id: string; size: string; kind: string; invalid: string[];
    budget_stop: boolean; omo: Metrics; pi_native: Metrics;
    raw: { path: string; sha256: string }[];
  }[] = [];
  for (const pair of manifest.pairs) {
    const paths = [pair.omo.show, pair.omo.events, pair.pi_native.show, pair.pi_native.events, pair.selection];
    const raw = await Promise.all(paths.map((path) => readRaw(resolve(root, path))));
    const [leftShow, leftEvents, rightShow, rightEvents] = raw;
    if (leftShow === undefined || leftEvents === undefined || rightShow === undefined || rightEvents === undefined) {
      throw new ComparisonError("raw_missing", pair.id);
    }
    const omo = showSchema.parse(JSON.parse(leftShow));
    const pi = showSchema.parse(JSON.parse(rightShow));
    const invalid = validatePair(pair, manifest.seed, omo, pi);
    const selectionRaw = raw[4];
    if (selectionRaw === undefined) throw new ComparisonError("selection_missing", pair.id);
    const selection = selectionSchema.parse(JSON.parse(selectionRaw));
    if (selection.id !== omo.id || selection.body !== omo.body
      || selection.kind !== omo.kind || selection.size !== omo.size) {
      invalid.push("Backlog selection changed");
    }
    if (used.has(omo.id)) invalid.push("task reused across pairs");
    used.add(omo.id);
    const left = measure(omo, selectedRun(omo, pair.omo.run), parseEvents(leftEvents));
    const right = measure(pi, selectedRun(pi, pair.pi_native.run), parseEvents(rightEvents));
    const first = pair.order[0] === "omo" ? left : right;
    const secondRun = pair.order[0] === "omo"
      ? selectedRun(pi, pair.pi_native.run) : selectedRun(omo, pair.omo.run);
    if (Date.parse(first.approved_at) > Date.parse(secondRun.started)) {
      invalid.push("arms overlap before independent approval");
    }
    if (left.api_equivalent_usd === null || right.api_equivalent_usd === null) {
      invalid.push("API-equivalent pricing incomplete");
    }
    if (left.review_rounds > 3 || right.review_rounds > 3) invalid.push("three-review-round budget exceeded");
    const budgetStop = left.total_tokens + right.total_tokens > 2 * pair.expected_tokens;
    pairs.push({
      id: pair.id, size: omo.size, kind: omo.kind, invalid, budget_stop: budgetStop,
      omo: left, pi_native: right,
      raw: paths.map((path, index) => {
        const content = raw[index];
        if (content === undefined) throw new ComparisonError("raw_missing", path);
        return { path, sha256: sha256(content) };
      }),
    });
  }
  const small = pairs.filter((pair) => pair.size === "xs" || pair.size === "s").length;
  const medium = pairs.filter((pair) => pair.size === "m").length;
  const large = pairs.filter((pair) => pair.size === "l").length;
  const complete = pairs.length === 6 && small === 2 && medium === 3 && large === 1
    && new Set(pairs.map((pair) => pair.kind)).size > 1
    && new Set(manifest.pairs.map((pair) => pair.id)).size === 6;
  const omo = aggregate(pairs.map((pair) => pair.omo));
  const pi = aggregate(pairs.map((pair) => pair.pi_native));
  const checks = thresholds(omo, pi);
  const stopped = pairs.some((pair) => pair.budget_stop);
  const valid = complete && pairs.every((pair) => !pair.invalid.length) && !stopped;
  const measured = manifest.data_kind === "measured";
  const pass = valid && measured && Object.values(checks).every((value) => value === true);
  return {
    version: 1, data_kind: manifest.data_kind, complete, valid, budget_stop: stopped,
    outcome: !valid || !measured ? "not_evaluated" : pass ? "pass" : "fail",
    pm_evaluated: false, defaults_changed: false,
    thresholds: checks, aggregates: { omo, pi_native: pi }, pairs,
  };
}
export type Report = Awaited<ReturnType<typeof compare>>;

export async function compareFile(path: string) {
  const raw = await readRaw(path);
  const manifest = manifestSchema.parse(JSON.parse(raw));
  return { ...await compare(manifest, dirname(resolve(path))), manifest_sha256: sha256(raw) };
}
