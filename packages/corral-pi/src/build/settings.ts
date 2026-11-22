import { z } from "zod";
import { parseBuildProfiles, readWorkspaceToml } from "./profiles.ts";
import { DEFAULT_MAX_PARALLEL, MAX_PARALLEL_LIMIT, type BuildSettings } from "./types.ts";

const nonBlank = z.string().refine(value => value.trim().length > 0, "must not be blank");
const Route = z.strictObject({
  match: nonBlank,
  kind: z.enum(["explore", "slice"]),
  profile: nonBlank,
  strategy: z.enum(["when_delegating", "prefer_child"]).default("when_delegating"),
});
const Delegation = z.strictObject({
  mode: z.enum(["standard", "proactive"]).default("standard"),
  routes: z.array(Route).default([]),
}).prefault({});
const Build = z.object({
  max_parallel: z.number().int().min(1).max(MAX_PARALLEL_LIMIT).default(DEFAULT_MAX_PARALLEL),
  profiles: z.unknown().optional(),
  delegation: Delegation,
}).prefault({});

export function readBuildSettings(binding: { workspaceRoot?: string; repoRoot: string },
  raw = readWorkspaceToml(binding.workspaceRoot ?? binding.repoRoot, "pi.toml")): BuildSettings {
  const result = Build.safeParse(raw.build);
  if (!result.success) {
    const issues = result.error.issues.map(issue => `${["build", ...issue.path].join(".")}: ${issue.message}`);
    throw new Error(`.corral-next/pi.toml: ${issues.join("; ")}`);
  }
  const { max_parallel, delegation } = result.data;
  const profiles = parseBuildProfiles(result.data.profiles);
  const names = new Set(["default", ...profiles.map(profile => profile.name)]);
  for (const [index, route] of delegation.routes.entries()) {
    if (!names.has(route.profile)) {
      throw new Error(`.corral-next/pi.toml: build.delegation.routes.${index}.profile: unknown Build child profile ${JSON.stringify(route.profile)}`);
    }
  }
  return { exitPolicy: binding.workspaceRoot ? "review" : "none", maxParallel: max_parallel, profiles, delegation };
}
