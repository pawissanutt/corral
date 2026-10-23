// Bun runtime, exact Pi pin and frozen-lock checks shared by every Bun probe entry point.
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { ProbeFailure, record } from "./credentials.mjs";

/**
 * The probe runs only on Bun; Node is not a supported host (T-252 non-goal).
 * @returns {{semver: {satisfies(version: string, range: string): boolean},
 * JSONC: {parse(text: string): unknown}}}
 */
export function bunApi() {
  const api = Reflect.get(globalThis, "Bun");
  if (!api || typeof process.versions.bun !== "string") throw new ProbeFailure("bun_runtime_required");
  return api;
}

/** @param {string} path */
export function readBunLock(path) {
  const bytes = readFileSync(path);
  const lock = bunApi().JSONC.parse(bytes.toString("utf8"));
  if (!record(lock) || !record(lock.packages) || lock.lockfileVersion !== 1 && lock.lockfileVersion !== 2) {
    throw new ProbeFailure("bun_lock_shape");
  }
  return { sha256: createHash("sha256").update(bytes).digest("hex"),
    packages: /** @type {Record<string, unknown>} */ (lock.packages),
    workspaces: /** @type {Record<string, Record<string, Record<string, string>>>} */ (lock.workspaces) };
}

/** @param {string} probeRoot */
export function readPins(probeRoot) {
  return JSON.parse(readFileSync(join(probeRoot, "pins.json"), "utf8"));
}

/**
 * Refuses an untested Bun, a lock other than the reviewed one, or an installed
 * Pi release whose version or integrity differs from the T-136/T-234 pin.
 * @param {string} probeRoot
 */
export function checkRuntime(probeRoot) {
  const pins = readPins(probeRoot);
  const version = /** @type {string} */ (process.versions.bun);
  if (!bunApi().semver.satisfies(version, pins.bunRange)) throw new ProbeFailure("bun_version_outside_tested_range");
  const lock = readBunLock(join(probeRoot, "bun.lock"));
  if (lock.sha256 !== pins.bunLockSha256) throw new ProbeFailure("bun_lock_digest_mismatch");
  const entry = lock.packages[pins.package];
  const manifest = JSON.parse(readFileSync(join(probeRoot, "node_modules", pins.package, "package.json"), "utf8"));
  if (!Array.isArray(entry) || entry[0] !== `${pins.package}@${pins.version}` || entry[3] !== pins.integrity ||
      manifest.version !== pins.version) {
    throw new ProbeFailure("runtime_pin_mismatch");
  }
  return { bun: version, bunRange: pins.bunRange, package: pins.package, version: pins.version,
    integrity: pins.integrity, upstreamCommit: pins.upstreamCommit, bunLockSha256: lock.sha256 };
}
