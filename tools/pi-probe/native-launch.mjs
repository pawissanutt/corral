import { spawn, spawnSync } from "node:child_process";
import { watch, writeFileSync, readFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { ProbeFailure, record } from "./credentials.mjs";
import { writeSyntheticAuth } from "./fake-transport.mjs";
import { liveAuthorization } from "./request-guard.mjs";
import { bunApi } from "./runtime.mjs";
import { CRITERIA } from "./native-scenarios.mjs";

const root = process.env.CORRAL_TEST_HERDR_ROOT;
const output = process.env.PI_PROBE_OUTPUT;
const live = process.env.PI_PROBE_NATIVE_OFFLINE === "0";
if (!root || process.env.HOME !== join(root, "home") || !output || (live && !process.env.PI_PROBE_CODEX_AUTH)) {
  throw new ProbeFailure("private_fixture_required");
}
bunApi();
if (live) {
  try {
    liveAuthorization(process.env.PI_PROBE_LIVE_AUTHORIZATION);
  } catch (error) {
    if (!(error instanceof ProbeFailure)) throw error;
    // Not a probe failure: without a new owner grant the live column stays NOT RUN.
    console.log(JSON.stringify({ type: "live_check_refused", reason: error.code, providerRequests: 0 }));
    writeFileSync(join(output, "native-live-result.json"), JSON.stringify({ mode: "live", decision: "NOT RUN",
      reason: error.code, providerRequests: 0 }, null, 2) + "\n");
    process.exit(3);
  }
}
// Node, npm and npx are shimmed to fail; the host must never need them.
for (const tool of ["node", "npm", "npx"]) {
  const probe = spawnSync(tool, ["--version"], { stdio: "ignore" });
  console.log(JSON.stringify({ type: "node_tool_unavailable", tool, status: probe.status }));
  if (probe.status !== 127) throw new ProbeFailure("node_shim_missing");
}
/** @param {string[]} args */
async function herdr(args) {
  return new Promise((resolve, reject) => {
    const child = spawn("herdr", ["--session", "corral-test", ...args], { stdio: ["ignore", "pipe", "ignore"] });
    let text = "";
    child.stdout.on("data", (chunk) => { text += chunk.toString(); });
    child.once("error", () => reject(new ProbeFailure("private_herdr_failed")));
    child.once("exit", (code) => code === 0 ? resolve(text) : reject(new ProbeFailure("private_herdr_failed")));
  });
}
// Offline uses a synthetic credential that only the fake transport accepts.
const codexPath = live ? /** @type {string} */ (process.env.PI_PROBE_CODEX_AUTH) : join(root, "synthetic-codex-auth.json");
if (!live) writeSyntheticAuth(codexPath, Date.now());
const created = JSON.parse(await herdr(["workspace", "create", "--cwd", root, "--label", "pi-native-probe", "--no-focus",
  "--env", "CORRAL_TEST_HERDR_ROOT=" + root,
  "--env", "PI_CODING_AGENT_DIR=" + join(root, "home/.pi/agent"),
  "--env", "PI_OFFLINE=1", "--env", "PI_SKIP_VERSION_CHECK=1", "--env", "PI_TELEMETRY=0",
  "--env", "PI_PROBE_OUTPUT=" + output,
  "--env", "PI_PROBE_LIVE_AUTHORIZATION=" + (live ? process.env.PI_PROBE_LIVE_AUTHORIZATION ?? "" : ""),
  "--env", "PI_PROBE_NATIVE_OFFLINE=" + (live ? "0" : "1")]));
const workspace = created.result?.workspace?.workspace_id ?? created.result?.root_pane?.workspace_id;
const pane = created.result?.root_pane?.pane_id;
if (typeof workspace !== "string" || typeof pane !== "string") throw new ProbeFailure("private_pane_shape");
writeFileSync(join(root, "native-bootstrap.json"), JSON.stringify({
  capability: randomUUID(), codexPath, workspace, pane,
  socket: join(process.env.XDG_CONFIG_HOME ?? "", "herdr/sessions/corral-test/herdr.sock"),
}) + "\n", { mode: 0o600 });
const resultPath = join(root, "native-result.json");
/** @type {ReturnType<typeof setTimeout> | undefined} */
let timer;
/** @type {import("node:fs").FSWatcher | undefined} */
let watcher;
const result = new Promise((resolve, reject) => {
  watcher = watch(root, (_event, filename) => {
    if (filename === "native-result.json" && existsSync(resultPath)) resolve(undefined);
  });
  timer = setTimeout(() => reject(new ProbeFailure("native_fixture_deadline")), 150_000);
});
/** @param {string} value */
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
const resultCopy = join(output, `native-${live ? "live" : "offline"}-result.json`);
try {
  const host = join(dirname(fileURLToPath(import.meta.url)), "native-host.mjs");
  await herdr(["pane", "run", pane, [process.execPath, host].map(quote).join(" ")]);
  await result;
  for (const line of readFileSync(join(root, "native-events.jsonl"), "utf8").trim().split("\n")) {
    const event = JSON.parse(line);
    if (!record(event)) throw new ProbeFailure("native_receipt_shape");
    console.log(JSON.stringify(event));
  }
  const final = JSON.parse(readFileSync(resultPath, "utf8"));
  console.log(JSON.stringify({ type: "native_result", ...final }));
  const install = join(dirname(fileURLToPath(import.meta.url)), "lock-check.json");
  writeFileSync(resultCopy, JSON.stringify({ ...final,
    install: existsSync(install) ? JSON.parse(readFileSync(install, "utf8")) : null }, null, 2) + "\n");
  if (final.decision === "NO-GO") process.exitCode = 2;
} catch (error) {
  const code = error instanceof ProbeFailure ? error.code : "native_launcher_error";
  console.log(JSON.stringify({ type: "native_launcher_stopped", origin: "harness_port", reason: code }));
  writeFileSync(resultCopy, JSON.stringify({ mode: live ? "live" : "offline_fake_transport", decision: "NO-GO",
    criteria: Object.fromEntries(CRITERIA.map((name) => [name, "NOT_RUN"])), allPassed: false,
    harnessPortFailures: [{ step: "launcher", origin: "harness_port", code }], upstreamIncompatibilities: [] }, null, 2) + "\n");
  process.exitCode = 2;
} finally {
  clearTimeout(timer);
  watcher?.close();
  await herdr(["workspace", "close", workspace]);
}
