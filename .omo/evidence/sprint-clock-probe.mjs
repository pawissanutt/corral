import assert from "node:assert/strict";
import { mkdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const [binaryArg, runtimeArg, libraryArg] = process.argv.slice(2);
const binary = resolve(binaryArg);
const runtime = resolve(runtimeArg);
const library = resolve(libraryArg);
const root = join(runtime, "board");
mkdirSync(root);
const env = {
  PATH: process.env.PATH,
  HOME: join(runtime, "home"),
  XDG_CONFIG_HOME: join(runtime, "config"),
  XDG_DATA_HOME: join(runtime, "data"),
  XDG_STATE_HOME: join(runtime, "state"),
  XDG_CACHE_HOME: join(runtime, "cache"),
  XDG_RUNTIME_DIR: join(runtime, "run"),
  TMPDIR: join(runtime, "tmp"),
  CORRAL_ROOT: root,
  HERDR_BIN_PATH: join(runtime, "no-herdr"),
  LD_PRELOAD: library,
};
for (const key of [
  "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME",
  "XDG_CACHE_HOME", "XDG_RUNTIME_DIR", "TMPDIR",
]) {
  mkdirSync(env[key], { recursive: true, mode: 0o700 });
}

async function cli(seconds, ...args) {
  const child = Bun.spawn([binary, ...args], {
    cwd: root,
    env: { ...env, SPRINT_FIXTURE_SECONDS: String(seconds) },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  console.log(JSON.stringify({ seconds, args, code, stdout, stderr }));
  assert.equal(code, 0, stderr);
  return stdout;
}

// Given a real owner-authored decline of a PM-filed idea at 00:00:10.
await cli(1767225600, "init");
await cli(1767225601, "new", "Filed idea", "--idea", "--origin", "pm");
await cli(1767225610, "idea", "1", "decline", "--reason", "No longer needed");
const events = readFileSync(join(root, ".corral/events.jsonl"), "utf8")
  .trim().split("\n").map(JSON.parse);
const decided = events.find((e) => e.kind === "idea_decided");
assert.equal(decided.ts, "2026-01-01T00:00:10Z");
assert.equal(decided.detail.actor, "owner");
console.log(JSON.stringify({ durable_decision: decided }));

// When only the review process's realtime moves backward, then forward.
for (const [seconds, expectedCount] of [
  [1767225609, 0],
  [1767225610, 1],
  [1767225611, 1],
  [1767225609, 0],
]) {
  const report = JSON.parse(await cli(seconds, "review-sprint", "--json"));
  const rows = report.sections.dropped_rejected_escalated;
  assert.equal(rows.length, expectedCount);
  if (expectedCount) {
    assert.equal(rows[0].actor, "owner");
    assert.equal(rows[0].reason, "No longer needed");
  }
  console.log(JSON.stringify({
    snapshot_at: report.snapshot_at,
    decision_at: decided.ts,
    outcome_count: rows.length,
    indexed_actor: rows[0]?.actor ?? null,
  }));
}
console.log("PASS: actual CLI reproduces empty outcome/Null only before decision timestamp");
