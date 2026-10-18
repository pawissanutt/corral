import { spawn } from "node:child_process";
import { watch, writeFileSync, readFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { ProbeFailure, record } from "./credentials.mjs";

const root = process.env.CORRAL_TEST_HERDR_ROOT;
if (!root || process.env.HOME !== join(root, "home") || !process.env.PI_PROBE_CODEX_AUTH) {
  throw new ProbeFailure("private_fixture_required");
}
/** @param {string[]} args */
async function herdr(args) {
  return new Promise((resolve, reject) => {
    const child = spawn("herdr", ["--session", "corral-test", ...args], { stdio: ["ignore", "pipe", "ignore"] });
    let output = "";
    child.stdout.on("data", (chunk) => { output += chunk.toString(); });
    child.once("error", () => reject(new ProbeFailure("private_herdr_failed")));
    child.once("exit", (code) => code === 0 ? resolve(output) : reject(new ProbeFailure("private_herdr_failed")));
  });
}
const created = JSON.parse(await herdr(["workspace", "create", "--cwd", root, "--label", "pi-native-probe", "--no-focus",
  "--env", "CORRAL_TEST_HERDR_ROOT=" + root,
  "--env", "PI_CODING_AGENT_DIR=" + join(root, "home/.pi/agent"),
  "--env", "PI_OFFLINE=1", "--env", "PI_SKIP_VERSION_CHECK=1", "--env", "PI_TELEMETRY=0",
  "--env", "PI_PROBE_NATIVE_OFFLINE=" + (process.env.PI_PROBE_NATIVE_OFFLINE ?? "1")]));
const workspace = created.result?.workspace?.workspace_id ?? created.result?.root_pane?.workspace_id;
const pane = created.result?.root_pane?.pane_id;
if (typeof workspace !== "string" || typeof pane !== "string") throw new ProbeFailure("private_pane_shape");
writeFileSync(join(root, "native-bootstrap.json"), JSON.stringify({
  capability: randomUUID(), codexPath: process.env.PI_PROBE_CODEX_AUTH, workspace, pane,
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
try {
  const host = join(dirname(fileURLToPath(import.meta.url)), "native-host.mjs");
  await herdr(["pane", "run", pane,
    [process.execPath, host].map(quote).join(" ")]);
  await result;
  for (const line of readFileSync(join(root, "native-events.jsonl"), "utf8").trim().split("\n")) {
    const event = JSON.parse(line);
    if (!record(event)) throw new ProbeFailure("native_receipt_shape");
    console.log(JSON.stringify(event));
  }
  const final = JSON.parse(readFileSync(resultPath, "utf8"));
  console.log(JSON.stringify({ type: "native_result", ...final }));
  if (final.decision === "no-go") process.exitCode = 2;
} catch (error) {
  console.log(JSON.stringify({ type: "native_launcher_stopped", reason:
    error instanceof ProbeFailure ? error.code : "native_launcher_error" }));
  process.exitCode = 2;
} finally {
  clearTimeout(timer);
  watcher?.close();
  await herdr(["workspace", "close", workspace]);
}
