import { randomUUID } from "node:crypto";
import { closeSync, constants, fstatSync, lstatSync, mkdirSync, openSync, readSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { corralPiRoot } from "../auth/paths.ts";
import { isProviderId } from "../auth/providers.ts";
import type { Appearance, Bell, Density, Motion } from "../ui/appearance.ts";

export interface PersonalPreferences {
  readonly appearance: Appearance;
  readonly density: Density;
  readonly motion: Motion;
  readonly bell: Bell;
  /** Only for a Build started outside a Corral workspace. */
  readonly startupModel: string | null;
}

export const DEFAULT_PERSONAL_PREFERENCES: PersonalPreferences = {
  appearance: "dark", density: "comfortable", motion: "animate", bell: "off", startupModel: null,
};

const keys = ["appearance", "density", "motion", "bell", "startupModel"] as const;
type Preference = typeof keys[number];
const FILE_LIMIT = 1024;

function valid(key: Preference, value: unknown): boolean {
  switch (key) {
    case "appearance": return value === "dark" || value === "light" || value === "contrast";
    case "density": return value === "comfortable" || value === "compact";
    case "motion": return value === "animate" || value === "still";
    case "bell": return value === "on" || value === "off";
    case "startupModel": {
      if (value === null) return true;
      if (typeof value !== "string" || Buffer.byteLength(value) > 256 || /[\s\p{Cc}]/u.test(value)) return false;
      const slash = value.indexOf("/");
      return slash > 0 && slash < value.length - 1 && isProviderId(value.slice(0, slash));
    }
  }
}

function readPreference(dir: string, key: Preference): unknown {
  const fd = openSync(join(dir, `${key}.json`), constants.O_RDONLY | constants.O_NONBLOCK | constants.O_NOFOLLOW);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > FILE_LIMIT) return undefined;
    const bytes = Buffer.alloc(FILE_LIMIT + 1);
    let count = 0;
    while (count < bytes.length) {
      const n = readSync(fd, bytes, count, bytes.length - count, null);
      if (n === 0) break;
      count += n;
    }
    return count <= FILE_LIMIT ? JSON.parse(bytes.subarray(0, count).toString("utf8")) : undefined;
  } finally { closeSync(fd); }
}

function preferencesDir(env: NodeJS.ProcessEnv): string {
  return join(corralPiRoot(env), "preferences");
}

/** Bad or missing local preferences cannot stop a launch; never read from a special file. */
export function readPersonalPreferences(env = process.env): PersonalPreferences {
  const saved = { ...DEFAULT_PERSONAL_PREFERENCES };
  try {
    const dir = preferencesDir(env);
    if (!lstatSync(dir).isDirectory()) return saved;
    for (const key of keys) {
      try {
        const value = readPreference(dir, key);
        if (valid(key, value)) Object.assign(saved, { [key]: value });
      } catch { /* leave the built-in presentation default */ }
    }
  } catch { /* missing or unavailable personal directory */ }
  return saved;
}

/** Independent atomic files avoid losing changes to unrelated keys made by another Pi host. */
export function savePersonalPreference<K extends Preference>(key: K, value: PersonalPreferences[K], env = process.env): void {
  if (!keys.includes(key) || !valid(key, value)) throw new Error("Invalid Corral Pi preference");
  const dir = preferencesDir(env);
  mkdirSync(dir, { mode: 0o700, recursive: true });
  if (!lstatSync(dir).isDirectory()) throw new Error("Corral Pi preferences path is not a directory");
  const temporary = join(dir, `.${randomUUID()}.tmp`);
  try {
    writeFileSync(temporary, `${JSON.stringify(value)}\n`, { mode: 0o600, flag: "wx" });
    renameSync(temporary, join(dir, `${key}.json`));
  } catch (error) {
    try { rmSync(temporary, { force: true }); } catch { /* preserve the original save error */ }
    throw error;
  }
}
