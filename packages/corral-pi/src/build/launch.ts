import { closeSync, readSync } from "node:fs";
import { isAbsolute } from "node:path";
import type { AwsBinding } from "../auth/aws.ts";
import { isAwsRegion, isProviderId } from "../auth/providers.ts";
import { capabilityGuidance, capabilitySet, LaunchMessageInvalid } from "../host/config.ts";
import { EXIT_POLICIES, type BuildLaunch, type ExitPolicy } from "./types.ts";

type Json = Record<string, unknown>;
const MAX_BYTES = 64 * 1024;
function object(value: unknown, key: string): Json {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new LaunchMessageInvalid(`${key} must be an object`);
  return value as Json;
}
function text(value: Json, key: string, max = 4096): string {
  const entry = value[key];
  if (typeof entry !== "string" || !entry.trim() || entry.length > max || /[\u0000-\u001f\u007f]/.test(entry)) {
    throw new LaunchMessageInvalid(`${key} is missing or malformed`);
  }
  return entry;
}
function path(value: Json, key: string): string {
  const entry = text(value, key);
  if (!isAbsolute(entry)) throw new LaunchMessageInvalid(`${key} must be an absolute path`);
  return entry;
}
function token(value: Json, key: string): string {
  const entry = text(value, key, 128);
  if (!/^[A-Za-z0-9._:+-]+$/.test(entry)) throw new LaunchMessageInvalid(`${key} is malformed`);
  return entry;
}
function model(value: Json, qualified: boolean): string {
  const entry = text(value, "model", 512);
  const slash = entry.indexOf("/");
  if (/\s/.test(entry) || (qualified && (slash < 1 || slash === entry.length - 1 || !isProviderId(entry.slice(0, slash))))) {
    throw new LaunchMessageInvalid("model must be <provider>/<id>");
  }
  return entry;
}

/** Reads at most 64 KiB of the inherited wire message, always closing its descriptor. */
export function readBuildLaunch(fd: number): BuildLaunch {
  const bytes = Buffer.alloc(MAX_BYTES + 1);
  let length = 0;
  try {
    while (length < bytes.length) {
      const read = readSync(fd, bytes, length, bytes.length - length, null);
      if (!read) break;
      length += read;
    }
  } finally { closeSync(fd); }
  if (length > MAX_BYTES) throw new LaunchMessageInvalid("exceeds its bound");
  let parsed: unknown;
  try { parsed = JSON.parse(bytes.subarray(0, length).toString("utf8")); }
  catch { throw new LaunchMessageInvalid("not JSON"); }
  const wire = object(parsed, "launch");
  if (wire.version !== 1) throw new LaunchMessageInvalid("version must be 1");
  if (typeof wire.here !== "boolean") throw new LaunchMessageInvalid("here must be a boolean");
  if (!isProviderId(wire.provider)) throw new LaunchMessageInvalid("provider is unknown");
  const settings = object(wire.settings, "settings");
  if (!EXIT_POLICIES.includes(settings.exit_policy as ExitPolicy)) throw new LaunchMessageInvalid("exit_policy is unknown");
  if (!Number.isSafeInteger(settings.max_parallel) || Number(settings.max_parallel) < 1 || Number(settings.max_parallel) > 8) {
    throw new LaunchMessageInvalid("max_parallel must be 1 to 8");
  }
  if (!Array.isArray(settings.profiles)) throw new LaunchMessageInvalid("profiles must be a list");
  const names = new Set<string>();
  const profiles = settings.profiles.map((entry) => {
    const profile = object(entry, "profile");
    const name = text(profile, "name", 128);
    if (names.has(name)) throw new LaunchMessageInvalid("profile names must be unique");
    names.add(name);
    return { name, model: model(profile, true), ...(profile.effort != null ? { effort: token(profile, "effort") } : {}),
      ...(profile.description != null ? { description: text(profile, "description") } : {}) };
  });
  const resume = wire.resume == null ? undefined : token(wire, "resume");
  if (resume !== undefined && !/^B-[a-z0-9]+$/.test(resume)) throw new LaunchMessageInvalid("resume is malformed");
  const project = wire.project == null ? undefined : token(wire, "project");
  if (project !== undefined && !/^[A-Za-z0-9._:+-]{1,128}$/.test(project)) throw new LaunchMessageInvalid("project is malformed");
  const capabilities = capabilitySet(wire.capabilities);
  const guidance = capabilityGuidance(wire.guidance ?? wire.capabilities);
  if (wire.guidance != null && (!guidance || Array.isArray(wire.guidance))) throw new LaunchMessageInvalid("guidance is malformed");
  let aws: AwsBinding | undefined;
  if (wire.aws != null) {
    const binding = object(wire.aws, "aws");
    // The bound launch uses the same non-secret aws: { profile, region } wire binding.
    const region = text(binding, "region", 32);
    if (!/^[a-z0-9-]+$/.test(region) || !isAwsRegion(region)) throw new LaunchMessageInvalid("region is missing or malformed");
    const profile = text(binding, "profile", 64);
    if (!/^[A-Za-z0-9_.+=,@-]{1,64}$/.test(profile)) throw new LaunchMessageInvalid("profile is missing or malformed");
    aws = { profile, region };
  }
  return {
    version: 1, assets: path(wire, "assets"), dataRoot: path(wire, "data_root"), corral: path(wire, "corral"),
    repoRoot: path(wire, "repo_root"), here: wire.here, provider: wire.provider, model: model(wire, false),
    ...(wire.workspace_root != null ? { workspaceRoot: path(wire, "workspace_root") } : {}),
    ...(project !== undefined ? { project } : {}), ...(wire.name != null ? { name: text(wire, "name", 256) } : {}),
    ...(resume ? { resume } : {}), ...(wire.effort != null ? { effort: token(wire, "effort") } : {}),
    ...(capabilities ? { capabilities } : {}), ...(guidance ? { guidance } : {}),
    ...(aws ? { aws } : {}),
    settings: { exitPolicy: settings.exit_policy as ExitPolicy, maxParallel: settings.max_parallel as number, profiles },
  };
}
