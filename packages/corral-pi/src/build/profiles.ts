import { readFileSync } from "node:fs";
import { join } from "node:path";
import { TOML } from "bun";
import { isProviderId } from "../auth/providers.ts";
import { RemovedModelOption } from "../host/config.ts";
import type { BuildProfile } from "./types.ts";

export function readWorkspaceToml(root: string, file: "pi.toml" | "corral.toml"): Record<string, unknown> {
  try { return TOML.parse(readFileSync(join(root, ".corral-next", file), "utf8")) as Record<string, unknown>; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
}

export function readBuildProfiles(root: string): BuildProfile[] {
  const { build } = readWorkspaceToml(root, "pi.toml") as { build?: { profiles?: unknown } };
  return parseBuildProfiles(build?.profiles);
}

export function parseBuildProfiles(profiles: unknown): BuildProfile[] {
  if (profiles === undefined) return [];
  if (!profiles || typeof profiles !== "object" || Array.isArray(profiles)) {
    throw new Error(".corral-next/pi.toml: build.profiles must be a table");
  }
  return Object.entries(profiles).map(([name, value]) => {
    const profile = value as (Partial<BuildProfile> & { connection?: unknown }) | undefined;
    if (profile?.connection !== undefined) {
      const provider = profile.connection === "openai-api" ? "openai" : "anthropic-api";
      throw new RemovedModelOption(`Build child profile ${name}: connection was removed; use model = "${provider}/<model>"`);
    }
    const model = typeof profile?.model === "string" && /^([^/]+)\/([^\s\0]+)$/.exec(profile.model);
    const provider = model && model[1];
    if (!model || !isProviderId(provider) || (profile?.effort !== undefined && typeof profile.effort !== "string")
      || (profile?.description !== undefined && typeof profile.description !== "string")) {
      throw new Error(`Invalid Build child profile ${name}: expected a provider/model and optional effort and description`);
    }
    return { name, model: profile!.model!, effort: profile!.effort, description: profile!.description };
  });
}

export function buildProfileArgs(root: string, name: string): string[] {
  const { profiles } = readWorkspaceToml(root, "corral.toml") as {
    profiles?: Record<string, { harness?: string; model?: string; effort?: string; args?: string[]; fallback?: unknown }>;
  };
  const profile = profiles?.[name];
  if (profile?.harness !== "pi") throw new Error(`Build profile ${name} must name a [profiles.${name}] entry with harness = "pi"`);
  if (profile.fallback !== undefined && (!Array.isArray(profile.fallback) || profile.fallback.some(value => typeof value !== "string"))) {
    throw new Error(`Build profile ${name}: fallback must be a list of <provider>/<model> strings`);
  }
  const fallbackArgs = profile.fallback === undefined ? []
    : profile.fallback.length === 0 ? ["--no-fallback"]
    : profile.fallback.flatMap(value => ["--fallback", value]);
  return [...(profile.model ? ["--model", profile.model] : []), ...(profile.effort ? ["--effort", profile.effort] : []),
    ...profile.args ?? [], ...fallbackArgs];
}
