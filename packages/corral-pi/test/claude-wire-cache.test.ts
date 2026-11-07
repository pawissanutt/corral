import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { readlinkSync } from "node:fs";
import { fileURLToPath } from "node:url";

// One real-SDK journey. No real credentials or external network; the PID namespace also ensures
// an interrupted fixture cannot leave a Claude Code child behind. Linux gate prerequisites are
// deliberate: missing unshare/ip/user namespaces fail instead of silently running online/skipping.
test.skipIf(process.platform !== "linux")("Claude's effective prefix survives edit, stage and commit", () => {
  const fixture = fileURLToPath(new URL("./support/claude-wire-cache.ts", import.meta.url));
  const child = spawnSync("unshare", ["--user", "--map-root-user", "--net", "--pid", "--fork", "--kill-child",
    "sh", "-c", 'ip link set lo up && exec "$@"', "claude-cache-fixture",
    process.execPath, fixture, readlinkSync("/proc/self/ns/net"),
    ...(process.env.CORRAL_TEST_CLAUDE_EXECUTABLE ? [process.env.CORRAL_TEST_CLAUDE_EXECUTABLE] : [])], {
    env: { PATH: process.env.PATH }, encoding: "utf8", input: "", timeout: 45_000, maxBuffer: 1024 * 1024,
  });
  if (child.status !== 0) throw new Error(child.stderr || String(child.error ?? child.signal));
  expect(child.stdout).toContain('"prefixStable":true');
  console.log(child.stdout.trim());
}, 60_000);
