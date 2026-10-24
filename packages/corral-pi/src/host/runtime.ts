// Startup pins: the host runs only on a Bun inside the GO manifest's range, from the frozen
// lock it names, with the exact Pi release that manifest proved.
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";

export interface Compat {
  readonly version: 1;
  readonly verdict: "GO";
  readonly bun: { readonly range: string };
  readonly pi: { readonly package: string; readonly version: string; readonly integrity: string };
  readonly lock: { readonly file: string; readonly sha256: string };
}

export class RuntimeRefused extends Error {
  constructor(readonly code: string, reason: string) {
    super(`${code}: ${reason}`);
  }
}

interface BunApi {
  readonly version: string;
  readonly semver: { satisfies(version: string, range: string): boolean };
  readonly JSONC: { parse(text: string): unknown };
}

export function bunApi(): BunApi {
  const api = Reflect.get(globalThis, "Bun") as BunApi | undefined;
  if (!api || typeof process.versions.bun !== "string") {
    throw new RuntimeRefused("bun_runtime_required", "the Pi host runs on Bun only");
  }
  return api;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function readCompat(assetRoot: string): Compat {
  let value: unknown;
  try {
    value = JSON.parse(readFileSync(join(assetRoot, "compat.json"), "utf8"));
  } catch {
    throw new RuntimeRefused("compat_unavailable", `no readable compat.json under ${assetRoot}`);
  }
  if (!record(value) || value.verdict !== "GO" || !record(value.bun) || !record(value.pi) || !record(value.lock) ||
      typeof value.bun.range !== "string" || typeof value.pi.package !== "string" ||
      typeof value.pi.version !== "string" || typeof value.pi.integrity !== "string" ||
      typeof value.lock.sha256 !== "string" || typeof value.lock.file !== "string") {
    throw new RuntimeRefused("compat_unavailable", "compat.json does not record a GO verdict with its pins");
  }
  return value as unknown as Compat;
}

export interface RuntimePins {
  readonly bun: string;
  readonly pi: string;
  readonly lockSha256: string;
}

export function checkRuntime(assetRoot: string, compat: Compat, bunVersion = bunApi().version): RuntimePins {
  const bun = bunApi();
  if (!bun.semver.satisfies(bunVersion, compat.bun.range)) {
    throw new RuntimeRefused("bun_version_unsupported",
      `bun ${bunVersion} is outside the tested range ${compat.bun.range}`);
  }
  const bytes = readFileSync(join(assetRoot, compat.lock.file));
  const lockSha256 = createHash("sha256").update(bytes).digest("hex");
  if (lockSha256 !== compat.lock.sha256) {
    throw new RuntimeRefused("lock_digest_mismatch", `${compat.lock.file} is not the GO lock`);
  }
  const lock = bun.JSONC.parse(bytes.toString("utf8"));
  const entry = record(lock) && record(lock.packages) ? lock.packages[compat.pi.package] : undefined;
  let installed: unknown;
  try {
    installed = JSON.parse(readFileSync(join(assetRoot, "node_modules", compat.pi.package, "package.json"), "utf8"));
  } catch {
    throw new RuntimeRefused("runtime_pin_mismatch", `${compat.pi.package} is not installed`);
  }
  if (!Array.isArray(entry) || entry[0] !== `${compat.pi.package}@${compat.pi.version}` ||
      entry[3] !== compat.pi.integrity || !record(installed) || installed.version !== compat.pi.version) {
    throw new RuntimeRefused("runtime_pin_mismatch", `${compat.pi.package} is not the pinned ${compat.pi.version}`);
  }
  return { bun: bunVersion, pi: compat.pi.version, lockSha256 };
}
