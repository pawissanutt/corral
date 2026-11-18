import pkg from "../../package.json" with { type: "json" };

interface BunApi {
  readonly semver: { satisfies(version: string, range: string): boolean };
}

/** Compiled executables already carry the runtime and frozen dependencies they were built with. */
export function checkRuntime(version = process.versions.bun): void {
  const bun = Reflect.get(globalThis, "Bun") as BunApi | undefined;
  if (!bun || typeof version !== "string") throw new Error("bun_runtime_required: corral-pi runs on Bun");
  const range = pkg.engines.bun;
  if (!bun.semver.satisfies(version, range)) {
    throw new Error(`bun_version_unsupported: Bun ${version} is outside ${range}; install Bun ${range}`);
  }
}
