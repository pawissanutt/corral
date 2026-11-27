import type { ThinkingLevel } from "@earendil-works/pi-agent-core";
import type { BuildProfile, BuildSettings } from "../../build/types.ts";

export interface ResolvedModel {
  readonly profile: string;
  readonly role?: string;
  readonly provider: string;
  readonly model: string;
  readonly effort?: string;
  readonly serviceTier?: "priority";
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
  return resolveChoice(name, profile, approved);
}

/** A Worker task may neither inherit its parent's model nor pass a raw model ID. */
export function resolveTaskProfile(choice: string | undefined, configured: readonly BuildProfile[], approved: readonly string[]): ResolvedModel {
  if (typeof choice !== "string" || !choice.trim()) throw new Error("A configured child profile is required");
  if (choice === "default") throw new Error('The "default" profile is not allowed for Worker task');
  const profile = configured.find(profile => profile.name === choice);
  if (!profile && choice.includes("/")) throw new Error("Use a named child profile, not a provider/model");
  if (!profile) throw new Error(`Unknown child profile: ${JSON.stringify(choice)}`);
  return resolveChoice(choice, profile, approved);
}

export function childThinking(effort?: string): ThinkingLevel {
  if (effort === undefined) return "off";
  if (!["off", "minimal", "low", "medium", "high", "xhigh", "max"].includes(effort)) throw new Error(`Unknown child effort: ${effort}`);
  return effort as ThinkingLevel;
}

function resolveChoice(name: string, profile: BuildProfile | undefined, approved: readonly string[]): ResolvedModel {
  const value = profile?.model ?? name;
  const slash = value.indexOf("/");
  if (slash < 1 || slash === value.length - 1) throw new Error(`Invalid child model ${value}; expected <provider>/<id>`);
  const provider = value.slice(0, slash);
  if (!approved.includes(provider)) throw new Error(`Child model provider is not approved: ${provider}`);
  const id = value.slice(slash + 1);
  // Profile model IDs may already carry the provider prefix.
  const model = profile && id.startsWith(`${provider}/`) ? id.slice(provider.length + 1) : id;
  return { profile: profile?.name ?? value, provider, model, effort: profile?.effort,
    ...(profile?.role ? { role: profile.role } : {}), ...(profile?.serviceTier ? { serviceTier: profile.serviceTier } : {}) };
}
