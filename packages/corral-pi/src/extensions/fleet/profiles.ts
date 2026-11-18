import type { BuildProfile, BuildSettings } from "../../build/types.ts";

export interface ResolvedModel {
  readonly connection?: BuildProfile["connection"];
  readonly profile: string;
  readonly provider: string;
  readonly model: string;
  readonly effort?: string;
}

export function resolveProfiles(settings: BuildSettings, parent: { provider: string; model: string; effort?: string; connection?: BuildProfile["connection"] }): BuildProfile[] {
  const profiles = settings.profiles.map(profile => ({ ...profile }));
  if (!profiles.some(profile => profile.name === "default")) {
    profiles.unshift({ name: "default", model: `${parent.provider}/${parent.model}`, effort: parent.effort, connection: parent.connection });
  }
  return profiles;
}

export function resolveModel(choice: string | undefined, profiles: readonly BuildProfile[], approved: readonly string[]): ResolvedModel {
  const name = choice ?? "default";
  const profile = profiles.find(profile => profile.name === name);
  if (!profile && !name.includes("/")) throw new Error(`Unknown child profile: ${name}`);
  const value = profile?.model ?? name;
  const slash = value.indexOf("/");
  if (slash < 1 || slash === value.length - 1) throw new Error(`Invalid child model ${value}; expected <provider>/<id>`);
  const provider = value.slice(0, slash);
  if (!profile?.connection && !approved.includes(provider)) throw new Error(`Child model provider is not approved: ${provider}`);
  const id = value.slice(slash + 1);
  // Profile model IDs may already carry the provider prefix.
  const model = profile && id.startsWith(`${provider}/`) ? id.slice(provider.length + 1) : id;
  return { profile: profile?.name ?? value, provider, model, effort: profile?.effort, ...(profile?.connection ? { connection: profile.connection } : {}) };
}
