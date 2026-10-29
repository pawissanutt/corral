// Focused, server-free compatibility check: bun scripts/qa/storage-compat.mjs CANDIDATE
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { fixtureWriter } from "./store-fixture.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binary = resolve(process.argv[2] ?? "target/debug/corral");
const root = await mkdtemp("/tmp/corral-qa-compat-");
const env = { ...process.env };
for (const key of Object.keys(env)) {
  if (key.startsWith("CORRAL_") || key.startsWith("HERDR_") ||
      ["AI_AGENT", "PI_CODING_AGENT"].includes(key)) delete env[key];
}
for (const [key, directory] of Object.entries({
  HOME: "home", XDG_CONFIG_HOME: "config", XDG_CACHE_HOME: "cache",
  XDG_DATA_HOME: "data", XDG_STATE_HOME: "state",
})) {
  env[key] = join(root, directory);
  await mkdir(env[key]);
}
async function command(argv, pass = true) {
  const child = Bun.spawn(argv, { cwd: root, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  if (pass) assert.equal(code, 0, `${argv.join(" ")}\n${stderr}`);
  return { stdout, stderr, code };
}
const cli = async (...args) => (await command([binary, "--root", root, ...args])).stdout;
const json = async (...args) => JSON.parse(await cli(...args));
const jsonl = text => text.trim().split("\n").filter(Boolean).map(JSON.parse);

try {
  // Given a real v2 store and its full CLI record, not a list summary.
  await cli("init");
  const id = (await cli("new", "Compatibility fixture", "--body", "Preserved acceptance")).trim();
  const writer = await fixtureWriter(root, env);
  const seed = await json("show", id, "--json");
  const at = "2026-10-17T00:00:00Z";
  const task = { ...seed, updated: at, dispatch_hold: true, runs: [{
    id: "R1", agent: "fixture", worktree: root, branch: "synthetic", base: "main",
    base_sha: "0".repeat(40), started: at,
    herdr: { workspace_id: "w9", pane_id: "w9:p1" }, agent_state: "working",
  }], comments: [{ author: "owner", at, text: "Preserved context" }] };
  // When storing a fixed timestamp without active usage collection.
  await writer.putTask({ ...task, runs: [] });
  // Then the exact fixture timestamp survives a CLI readback.
  assert.equal((await json("show", id, "--json")).updated, at);
  // When the same hooks store active synthetic history (show may now refresh usage).
  await writer.putTask(task);
  // Then show retains that history and the summary is not mistaken for a full task.
  const full = await json("show", id, "--json");
  assert.equal(full.body, task.body);
  assert.equal(full.runs[0].herdr.pane_id, "w9:p1");
  assert.deepEqual(full.comments, task.comments);
  const summaries = await json("list", "--all", "--json");
  assert.equal(summaries.length, 1);
  assert.equal(summaries[0].runs, undefined);
  console.log("PASS: exact task history, body, timestamp, and summary/show separation");

  // Given historical timeline input including noise and other-task/global events.
  const scenario = (await readFile(join(repo, "crates/corral-ops/tests/fixtures/task-history.jsonl"),
    "utf8")).replaceAll("T-123", id);
  // When replacing the fixture's event history through the v2 API.
  await writer.replaceEvents(scenario);
  // Then raw Event and wrapped TimelineEntry JSONL agree without losing event detail.
  const raw = jsonl(await cli("export", "events", "--json"));
  assert.equal(raw.length, jsonl(scenario).length);
  const timeline = jsonl(await cli("log", id, "--all", "--json"));
  assert.deepEqual(timeline.map(entry => entry.event), raw.filter(event => event.task === id));
  assert.equal(timeline.find(entry => entry.kind === "replied").wait.duration.secs, 3900);
  assert.equal(timeline.find(entry => entry.kind === "blocked").wait.open, true);
  const last = raw.at(-1).seq;
  console.log("PASS: raw Event versus TimelineEntry JSONL, historical waits, and noise isolation");

  // Given a nonempty event stream, when a new fixture event is appended.
  const appended = { ts: at, task: id, kind: "commented", actor: "owner", via: "cli",
    detail: { text: "Exact appended event" } };
  await writer.appendEvents(JSON.stringify(appended) + "\n");
  // Then sequence-based export sees only the new raw event, with its exact timestamp.
  const newer = jsonl(await cli("export", "events", "--since", String(last), "--json"));
  assert.equal(newer.length, 1);
  assert.deepEqual({ ...newer[0], seq: undefined }, { ...appended, seq: undefined });
  console.log("PASS: appended fixture event and sequence cursor");

  // Given the same fixture, when the event history is intentionally cleared.
  await writer.replaceEvents("");
  // Then it is genuinely empty, rather than silently falling back to stale input.
  assert.equal(await cli("export", "events", "--json"), "");
  assert.deepEqual(jsonl(await cli("log", id, "--all", "--json")), []);
  console.log("PASS: empty fixture event history");

  // Given an active run stored only in v2, when building a fake herdr inventory.
  const inventory = join(root, "inventory");
  await command(["python3", "-I", join(repo, "scripts/qa/fake_herdr_inventory.py"), binary, root, inventory]);
  // Then full show readbacks, not stripped summaries, supply its pane.
  const panes = JSON.parse(await readFile(join(inventory, "panes.json"), "utf8")).panes;
  assert.equal(panes.length, 1);
  assert.equal(panes[0].pane_id, "w9:p1");
  assert.equal(panes[0].agent_status, "working");
  console.log("PASS: fake herdr inventory from v2 list plus show");

  // Given a fresh v2 store with no legacy artifact directory, when it is backed up.
  const backup = join(root, "backup");
  await cli("storage", "backup", backup, "--json");
  // Then the offline copy opens and preserves its task without rsyncing SQLite/WAL files.
  const copied = JSON.parse((await command([binary, "--root", backup, "show", id, "--json"])).stdout);
  assert.equal(copied.body, task.body);
  assert.deepEqual(copied.runs, full.runs);
  assert.deepEqual(copied.comments, task.comments);
  const checked = JSON.parse((await command([binary, "--root", backup, "storage", "check", "--json"])).stdout);
  assert.deepEqual(checked.database, []);
  assert.deepEqual(checked.blobs, []);
  console.log("PASS: consistent offline v2 backup and storage check");

  // Given a valid task, when its v2 head is deliberately corrupted.
  await writer.corruptTask(id);
  // Then the real CLI fails meaningfully; no Markdown-only corruption is being ignored.
  const broken = await command([binary, "--root", root, "show", id, "--json"], false);
  assert.notEqual(broken.code, 0);
  assert.match(broken.stderr, /qa-invalid-status/);
  console.log("PASS: corrupt v2 task produces a real CLI decode failure");

  // Given that broken task, when it is removed by the fixture API.
  await writer.removeTask(id);
  // Then the CLI can read the empty board again.
  assert.deepEqual(await json("list", "--all", "--json"), []);
  console.log("PASS: fixture removal restores an empty board");

  // Given a registered fixture, when its safety marker is no longer valid.
  await writeFile(join(root, ".corral-qa-fixture"), "not-synthetic\n");
  // Then the helper refuses without mutating the board.
  await assert.rejects(writer.replaceEvents(""), /synthetic QA fixture/);
  console.log("PASS: fixture safety marker refusal");
} finally {
  await rm(root, { recursive: true, force: true });
}
