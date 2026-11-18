import type { BuildProfile, BuildSettings } from "../../build/types.ts";

export interface ResolvedModel {
  readonly binding?: BuildProfile["binding"];
  readonly profile: string;
  readonly provider: string;
  readonly model: string;
  readonly effort?: string;
}

export function resolveProfiles(settings: BuildSettings, parent: { provider: string; model: string; effort?: string }): BuildProfile[] {
  const profiles = settings.profiles.map(profile => ({ ...profile }));
  if (!profiles.some(profile => profile.name === "default")) {
    profiles.unshift({ name: "default", model: `${parent.provider}/${parent.model}`, effort: parent.effort });
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
  if (!profile?.binding && !approved.includes(provider)) throw new Error(`Child model provider is not approved: ${provider}`);
  const id = value.slice(slash + 1);
  // The exact wire binding may retain the provider prefix; the API catalog indexes bare IDs.
  const model = profile?.binding && id.startsWith(`${provider}/`) ? id.slice(provider.length + 1) : id;
  return { profile: profile?.name ?? value, provider, model, effort: profile?.effort, ...(profile?.binding ? { binding: profile.binding } : {}) };
}
