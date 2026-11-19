import { readFileSync } from "node:fs";
import { join } from "node:path";
import { TOML } from "bun";
import { CONNECTIONS, isApiConnection, isConnectionId, resolveConnection } from "../auth/connections.ts";
import { isProviderId } from "../auth/providers.ts";
import type { BuildProfile } from "./types.ts";

export function readWorkspaceToml(root: string, file: "pi.toml" | "corral.toml"): Record<string, unknown> {
  try { return TOML.parse(readFileSync(join(root, ".corral-next", file), "utf8")) as Record<string, unknown>; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
}

export function readBuildProfiles(root: string): BuildProfile[] {
  const { build } = readWorkspaceToml(root, "pi.toml") as { build?: { profiles?: Record<string, unknown> } };
  return Object.entries(build?.profiles ?? {}).map(([name, value]) => {
    const profile = value as Partial<BuildProfile> | undefined;
    const model = typeof profile?.model === "string" && /^([^/]+)\/([^\s\0]+)$/.exec(profile.model);
    const provider = model && model[1];
    if (!model || !isProviderId(provider) || (profile?.effort !== undefined && typeof profile.effort !== "string")
      || (profile?.description !== undefined && typeof profile.description !== "string")) {
      throw new Error(`Invalid Build child profile ${name}: expected a provider/model and optional effort, description and API connection`);
    }
    const selected = profile?.connection;
    if (selected !== undefined && (!isConnectionId(selected) || !isApiConnection(selected) || CONNECTIONS[selected].provider !== provider)) {
      throw new Error(`Invalid API connection for Build child profile ${name}`);
    }
    const connection = resolveConnection(provider, selected);
    return { name, model: profile!.model!, effort: profile!.effort, description: profile!.description,
      ...(isApiConnection(connection) ? { connection } : {}) };
  });
}

export function buildProfileArgs(root: string, name: string): string[] {
  const { profiles } = readWorkspaceToml(root, "corral.toml") as {
    profiles?: Record<string, { harness?: string; model?: string; effort?: string; args?: string[] }>;
  };
  const profile = profiles?.[name];
  if (profile?.harness !== "pi") throw new Error(`Build profile ${name} must name a [profiles.${name}] entry with harness = "pi"`);
  return [...(profile.model ? ["--model", profile.model] : []), ...(profile.effort ? ["--effort", profile.effort] : []), ...profile.args ?? []];
}
