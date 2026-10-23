// Builds the bun-reproof compatibility record (bun-go.json) from the committed
// pins and lock plus a native offline result. GO requires a clean frozen
// install, all six criteria PASS through the fake transport with zero provider
// requests, and no harness-port or upstream failure. Offline and live proof
// stay in separate columns; live is NOT RUN without its own owner grant.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readBunLock, readPins } from "./runtime.mjs";
import { CRITERIA } from "./native-scenarios.mjs";

/** @param {Record<string, any>} offline @param {Record<string, any> | null} live */
export function goRecord(offline, live) {
  const probeRoot = dirname(fileURLToPath(import.meta.url));
  const pins = readPins(probeRoot);
  const lock = readBunLock(join(probeRoot, "bun.lock"));
  const install = offline.install ?? {};
  const results = Object.fromEntries(CRITERIA.map((name) => [name, offline.criteria?.[name] ?? "NOT_RUN"]));
  const go = offline.mode === "offline_fake_transport" && offline.decision === "GO" && offline.providerRequests === 0 &&
    CRITERIA.every((name) => results[name] === "PASS") && install.ok === true && install.lockSha256 === lock.sha256 &&
    offline.runtime?.bunLockSha256 === lock.sha256 && lock.sha256 === pins.bunLockSha256 &&
    (offline.harnessPortFailures ?? []).length === 0 && (offline.upstreamIncompatibilities ?? []).length === 0;
  /** @type {Record<string, string>} */
  const internal = {};
  for (const [name, value] of Object.entries(lock.packages)) {
    if (name.startsWith("@earendil-works/")) internal[name] = String(/** @type {unknown[]} */ (value)[0]).slice(name.length + 1);
  }
  return {
    version: 1,
    task: "T-298",
    plan: "T-252@P5",
    verdict: go ? "GO" : "NO-GO",
    bun: { tested: [offline.runtime?.bun], range: pins.bunRange },
    pi: { package: pins.package, version: pins.version, integrity: pins.integrity,
      upstreamCommit: pins.upstreamCommit, packages: internal },
    lock: { file: "tools/pi-probe/bun.lock", sha256: lock.sha256, t136NpmLockSha256: pins.t136LockSha256,
      diff: "tools/pi-probe/lock-diff.json" },
    install: { command: "bun install --frozen-lockfile --ignore-scripts", emptyRoot: true, ok: install.ok === true,
      lockUnchanged: install.lockUnchanged === true, locked: install.locked, installed: install.installed,
      runtimeCopies: install.runtimeCopies, peersSatisfied: (install.peers ?? []).every((/** @type {{satisfied: boolean}} */ peer) => peer.satisfied),
      hostProvidedPeers: (install.peers ?? []).filter((/** @type {{providedBy: string}} */ peer) => peer.providedBy === "root") },
    results,
    offline: { status: go ? "PASS" : "FAIL", transport: "fake_codex_sse", credential: "synthetic",
      fixture: "scripts/test-herdr.sh private plugin-free herdr server", nodeNpmNpx: "shimmed, exit 127",
      providerRequests: offline.providerRequests, transportRequests: offline.transportRequests,
      transportAborted: offline.transportAborted, transportFailures: offline.transportFailures,
      steps: (offline.steps ?? []).map((/** @type {{name: string, status: string}} */ step) => ({ name: step.name, status: step.status })),
      harnessPortFailures: offline.harnessPortFailures ?? [], upstreamIncompatibilities: offline.upstreamIncompatibilities ?? [] },
    live: live?.mode === "live" && live.decision === "bounded-native-proof"
      ? { status: "PASS", results: live.criteria, authorization: live.authorization ?? null }
      : { status: "NOT RUN", reason: live?.reason ?? "no owner authorization recorded for a live check",
        limit: "a separate owner grant no larger than T-136's: 5 requests, 1,024 output tokens each, $0 API" },
    fixtures: { usage: "tools/pi-probe/fixtures/session-usage.jsonl",
      usageExpected: "tools/pi-probe/fixtures/session-usage.expected.json",
      dangling: "tools/pi-probe/fixtures/session-dangling.jsonl",
      danglingExpected: "tools/pi-probe/fixtures/session-dangling.expected.json" },
  };
}

if (import.meta.main) {
  const [offlinePath, livePath] = process.argv.slice(2);
  if (!offlinePath) throw new Error("usage: bun go-record.mjs <native-offline-result.json> [<native-live-result.json>]");
  const live = livePath && existsSync(livePath) ? JSON.parse(readFileSync(livePath, "utf8")) : null;
  console.log(JSON.stringify(goRecord(JSON.parse(readFileSync(offlinePath, "utf8")), live), null, 2));
}
