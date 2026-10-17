// This is an offline prerequisite probe, not a native PM implementation.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const pins = JSON.parse(readFileSync(new URL("./pins.json", import.meta.url), "utf8"));
const fixture = resolve(process.argv[2]);
const root = process.env.CORRAL_TEST_HERDR_ROOT;
const home = process.env.HOME;
assert(home, "private HOME is required");
assert(root && fixture === join(root, "pi"), "requires the disposable herdr fixture");
assert.equal(process.version, pins.node);
assert.equal(process.env.PI_OFFLINE, "1");
assert.equal(process.env.PI_CODING_AGENT_DIR, join(home, ".pi/agent"));
assert(!process.env.HERDR_SOCKET_PATH && !process.env.HERDR_SESSION);
assert.equal(statSync(root).mode & 0o777, 0o700);

/** @param {string} type @param {Record<string, unknown>} data */
const receipt = (type, data) => console.log(JSON.stringify({ type, ...data }));
const pkgRoot = join(fixture, "node_modules", "@earendil-works/pi-coding-agent");
const manifest = JSON.parse(readFileSync(join(pkgRoot, "package.json"), "utf8"));
const lock = JSON.parse(readFileSync(join(fixture, "package-lock.json"), "utf8"));
const locked = lock.packages["node_modules/@earendil-works/pi-coding-agent"];
assert.equal(manifest.version, pins.version);
assert.equal(locked.integrity, pins.integrity);
receipt("package", {
  name: manifest.name, version: manifest.version, node: process.version,
  engine: manifest.engines.node, integrity: locked.integrity,
  lockSha256: createHash("sha256").update(JSON.stringify(lock)).digest("hex"),
});

const cli = spawnSync(process.execPath, [join(pkgRoot, manifest.bin.pi), "--version"], {
  cwd: root, env: process.env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
  timeout: 30_000, maxBuffer: 65_536,
});
assert.ifError(cli.error);
assert.equal(cli.status, 0, cli.stderr);
receipt("cli_version", { exit: cli.status, stdout: cli.stdout.trim() });

// Fail any attempted JS fetch. No login, checkAuth, refresh, prompt, stream,
// createAgentSession or credential resolution is invoked by this program.
let networkAttempts = 0;
globalThis.fetch = async () => {
  networkAttempts++;
  throw new Error("network forbidden in offline prerequisite probe");
};
const sdk = await import(pathToFileURL(join(pkgRoot, "dist/index.js")).href);
const authPath = join(process.env.PI_CODING_AGENT_DIR, "auth.json");
assert.equal(existsSync(authPath), false);
const runtime = await sdk.ModelRuntime.create({
  authPath, modelsPath: null, allowModelNetwork: false, refreshOnCreate: false,
});
const provider = runtime.getProvider(pins.provider);
assert(provider, "selected provider must exist");
const model = runtime.getModel(pins.provider, pins.model);
assert(model, "selected model must exist");
receipt("catalog", {
  provider: provider.id, model: model.id, api: model.api,
  contextWindow: model.contextWindow, maxTokens: model.maxTokens,
  subscription: provider.auth?.oauth?.isSubscription,
});
assert.equal(provider.auth?.oauth?.isSubscription, true);
const stored = await runtime.listCredentials();
assert.equal(stored.length, 0, "this probe must never consume owner credentials");
assert.equal(statSync(authPath).mode & 0o777, 0o600);
assert.equal(networkAttempts, 0);
receipt("prerequisite", {
  authenticated: false, credentialCount: stored.length, networkAttempts,
  decision: "no-go", reason: "isolated owner native sign-in not provisioned",
  liveCalls: 0, generatedTokens: 0, apiSpendUsd: 0,
});
receipt("coverage", {
  nativeTerminalControl: "not_run", busyInput: "not_run", cancellation: "not_run",
  lifecycleReceipts: "not_run", sessionOwnership: "not_run", restrictedPmTools: "not_run",
});
