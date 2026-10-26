import { afterEach, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { compare, compareFile, validatePair } from "../src/compare.ts";
import { assignedOrder, manifestSchema, showSchema } from "../src/schema.ts";
import type { Manifest } from "../src/schema.ts";

const fixtureRoot = new URL("./fixtures/", import.meta.url).pathname;
const manifest = manifestSchema.parse(JSON.parse(await readFile(join(fixtureRoot, "manifest.json"), "utf8")));
const show = showSchema.parse(JSON.parse(await readFile(join(fixtureRoot, "show.json"), "utf8")));
const rawEvents = await readFile(join(fixtureRoot, "events.jsonl"), "utf8");
const first = manifest.pairs[0];
if (!first) throw new TypeError("fixture pair absent");
const pair = first;
const roots: string[] = [];
afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true })));
});

async function sixPairs(): Promise<{ manifest: Manifest; root: string }> {
  const root = await mkdtemp(join(tmpdir(), "pi-compare-"));
  roots.push(root);
  const pairs = [];
  let candidate = 0;
  for (const [index, size] of ["s", "xs", "m", "m", "m", "l"].entries()) {
    // These generated fixture names select the existing raw fixture's dispatch order.
    while (assignedOrder(manifest.seed, `pair-${candidate}`)[0] !== "omo") candidate++;
    const id = `pair-${candidate++}`;
    const task = `T-synthetic-${index}`;
    const kind = index % 2 ? "bug" : "logic";
    const data = { ...show, id: task, size, kind };
    const selection = { id: task, body: data.body, size, kind, status: "backlog" };
    const showPath = `${index}-show.json`;
    const eventsPath = `${index}-events.jsonl`;
    const selectionPath = `${index}-selection.json`;
    await writeFile(join(root, showPath), JSON.stringify(data));
    await writeFile(join(root, eventsPath), rawEvents.replaceAll("T-synthetic", task));
    await writeFile(join(root, selectionPath), JSON.stringify(selection));
    pairs.push({ ...pair, id, selection: selectionPath,
      omo: { ...pair.omo, show: showPath, events: eventsPath },
      pi_native: { ...pair.pi_native, show: showPath, events: eventsPath },
    });
  }
  return { manifest: manifestSchema.parse({ ...manifest, pairs }), root };
}

test("partial synthetic raw data cannot become a measured pass", async () => {
  // Given the committed one-pair offline fixture.
  const path = join(fixtureRoot, "manifest.json");
  // When the real file-based pipeline is run.
  const actual = await compareFile(path);
  // Then all real acceptance claims remain false.
  expect(actual.complete).toBe(false);
  expect(actual.outcome).toBe("not_evaluated");
  expect(actual.pm_evaluated).toBe(false);
  expect(actual.defaults_changed).toBe(false);
  expect(actual.pairs[0]?.invalid).toEqual([]);
  expect(actual.pairs[0]?.raw).toHaveLength(5);
});

test("six synthetic pairs aggregate correctly without pretending to be live evidence", async () => {
  // Given the required size mix and mixed task kinds in isolated raw fixtures.
  const input = await sixPairs();
  // When all six pairs are compared.
  const actual = await compare(input.manifest, input.root);
  // Then the arithmetic passes but the run remains explicitly synthetic.
  expect(actual.complete).toBe(true);
  expect(actual.valid).toBe(true);
  expect(actual.outcome).toBe("not_evaluated");
  expect(actual.aggregates.omo.review_rounds).toBe(6);
  expect(actual.aggregates.pi_native.total_tokens).toBe(1290);
  expect(actual.aggregates.pi_native.api_equivalent_usd).toBeCloseTo(7.2);
  expect(Object.values(actual.thresholds)).toEqual([true, true, true, true, true]);
});

test("owner cost threshold fails even when subscription reports zero", async () => {
  // Given six pairs and one more expensive native worker.
  const input = await sixPairs();
  const nativePath = join(input.root, "0-show.json");
  const data = showSchema.parse(JSON.parse(await readFile(nativePath, "utf8")));
  const more = { ...data, runs: data.runs.map((run) => run.harness === "pi-native"
    ? { ...run, usage: { input: 100, output: 20, cache_read: 30, cache_write: 10,
      estimated_usd: 10, reported_usd: 0, unpriced_tokens: 0 } } : run) };
  await writeFile(nativePath, JSON.stringify(more));
  // When API-equivalent costs are compared.
  const actual = await compare(input.manifest, input.root);
  // Then zero reported billing does not bypass the 1.25x threshold.
  expect(actual.thresholds.api_equivalent_cost).toBe(false);
});

test("binary or config drift invalidates a pair before thresholds can endorse it", () => {
  // Given an after snapshot with a different installed binary digest.
  const changed = { ...pair, pi_native: { ...pair.pi_native,
    after: { ...pair.pi_native.after, binary_sha256: "c".repeat(64) } } };
  // When pair controls are validated.
  const actual = validatePair(changed, manifest.seed, show, show);
  // Then the pair is invalid rather than a failed worker result.
  expect(actual).toContain("binary/config/contract/reviewer/PM/slot identity drift");
});

test("effort, base, gate contract and independent reviewer mismatches are rejected", () => {
  // Given a native arm changing each frozen field.
  const changed = { ...show, gate_contract: { tier: "full" }, runs: show.runs.map((run) =>
    run.harness === "pi-native" ? { ...run, base_sha: "other", effort: "high",
      review: { reviewer: "owner", verdict: "approve" } } : run) };
  // When controls are validated.
  const actual = validatePair(pair, manifest.seed, show, changed);
  // Then every distinct source of invalidity is retained.
  expect(actual).toContain("base differs");
  expect(actual).toContain("effort differs");
  expect(actual).toContain("pi-native: gate contract digest mismatch");
  expect(actual).toContain("pi-native: independent reviewer profile mismatch");
});

test("twice-expected token budget stops an otherwise valid pair", async () => {
  // Given a preregistered expected pair budget of 100 tokens.
  const limited = { ...manifest, pairs: [{ ...pair, expected_tokens: 100 }] };
  // When the known 430-token pair is measured.
  const actual = await compare(limited, fixtureRoot);
  // Then the stop rule is explicit and excludes a pass.
  expect(actual.budget_stop).toBe(true);
  expect(actual.valid).toBe(false);
});

test("PM host is structurally required to remain OmO", () => {
  // Given a manifest attempting to evaluate a Pi PM as part of this comparison.
  const changed = { ...manifest, pairs: [{ ...pair, omo: { ...pair.omo,
    before: { ...pair.omo.before, pm_harness: "pi-native" } } }] };
  // When boundary parsing runs.
  const actual = manifestSchema.safeParse(changed);
  // Then the unsupported PM evaluation cannot enter measurement.
  expect(actual.success).toBe(false);
});

test("a lower native gate pass rate blocks the owner pass bar", async () => {
  // Given six fixtures and a native gate that failed before a later approval.
  const input = await sixPairs();
  const path = join(input.root, "0-events.jsonl");
  const events = await readFile(path, "utf8");
  await writeFile(path, events.replace('"attempt":"ag-2"},"result":"pass"', '"attempt":"ag-2"},"result":"fail"'));
  // When the captured event results are aggregated.
  const actual = await compare(input.manifest, input.root);
  // Then approval does not hide a lower gate success rate.
  expect(actual.thresholds.gate_pass_rate).toBe(false);
});

test("an extra native needs-me and silent ending each fail their own threshold", async () => {
  // Given six fixtures with one extra native hold and silent turn.
  const input = await sixPairs();
  const eventsPath = join(input.root, "0-events.jsonl");
  await writeFile(eventsPath, `${await readFile(eventsPath, "utf8")}\n${JSON.stringify({
    ts: "2026-10-24T01:00:19Z", task: "T-synthetic-0", kind: "needs_me", detail: { run: "R2" },
  })}\n`);
  const showPath = join(input.root, "0-show.json");
  const data = showSchema.parse(JSON.parse(await readFile(showPath, "utf8")));
  await writeFile(showPath, JSON.stringify({ ...data, runs: data.runs.map((run) =>
    run.harness === "pi-native" ? { ...run, agent_host: { turns: [
      ...(run.agent_host?.turns ?? []),
      { generation: 1, seq: 3, ended: "2026-10-24T01:00:38Z", class: "no_report" },
    ] } } : run) }));
  // When the complete cohort is compared.
  const actual = await compare(input.manifest, input.root);
  // Then neither owner requirement is replaced by the cost result.
  expect(actual.thresholds.needs_me).toBe(false);
  expect(actual.thresholds.turns_ended_without_report).toBe(false);
});

test("more than three independent review rounds triggers a budget invalidation", async () => {
  // Given one native run with four actual recorded reviewer children and verdicts.
  const input = await sixPairs();
  const path = join(input.root, "0-show.json");
  const data = showSchema.parse(JSON.parse(await readFile(path, "utf8")));
  await writeFile(path, JSON.stringify({ ...data, runs: data.runs.map((run) => {
    if (run.harness !== "pi-native") return run;
    const child = run.review_runs[0];
    if (!child) throw new TypeError("fixture child missing");
    return { ...run, review_runs: Array.from({ length: 4 }, (_, index) => ({
      ...child, id: `R2-review${index + 1}`,
    })) };
  }) }));
  const eventsPath = join(input.root, "0-events.jsonl");
  const extra = [45, 46, 47].map((second) => JSON.stringify({
    ts: `2026-10-24T01:00:${second}Z`, task: "T-synthetic-0", kind: "verdict",
    detail: { run: "R2", verdict: "changes", reviewer: "independent", sha: `candidate-${second}` },
  })).join("\n");
  await writeFile(eventsPath, `${await readFile(eventsPath, "utf8")}\n${extra}\n`);
  // When the cohort is compared.
  const actual = await compare(input.manifest, input.root);
  // Then both the live round cap and aggregate +1 threshold reject it.
  expect(actual.pairs[0]?.invalid).toContain("three-review-round budget exceeded");
  expect(actual.thresholds.review_rounds).toBe(false);
});

test("CLI exposes an incomplete synthetic report with non-pass exit status", async () => {
  // Given the real Bun CLI and committed offline raw fixtures.
  const args = [process.execPath, join(import.meta.dir, "../src/main.ts"), join(fixtureRoot, "manifest.json")];
  // When it is executed without any stdin or provider access.
  const child = Bun.spawn(args, { stdin: "ignore", stdout: "pipe", stderr: "pipe",
    signal: AbortSignal.timeout(5000) });
  const [status, stdout, stderr] = await Promise.all([
    child.exited, new Response(child.stdout).text(), new Response(child.stderr).text(),
  ]);
  // Then the actual surface reports data but never advertises acceptance.
  expect(status).toBe(2);
  expect(stderr).toBe("");
  expect(JSON.parse(stdout).outcome).toBe("not_evaluated");
});
